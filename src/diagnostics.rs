use serde::{Deserialize, Serialize};

use crate::diff_utils::apply_diff;
use crate::notebook::CellExt;
use crate::shas::{cell_state, CellState};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum DiagnosticType {
    /// Code cell has no ipso setup at all, or has ipso but no shas.
    Missing,
    /// This cell's own content changed since last accept.
    NeedsReview,
    /// A preceding cell was modified, deleted, inserted, or reordered.
    AncestorModified,
    /// The cell's diff does not apply cleanly to its current source.
    DiffConflict,
    /// Metadata validation failure (missing required field, bad value, etc.).
    InvalidField,
}

impl std::fmt::Display for DiagnosticType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = match self {
            DiagnosticType::Missing => "missing",
            DiagnosticType::NeedsReview => "needs_review",
            DiagnosticType::AncestorModified => "ancestor_modified",
            DiagnosticType::DiffConflict => "diff_conflict",
            DiagnosticType::InvalidField => "invalid_field",
        };
        write!(f, "{}", s)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum Severity {
    Error,
    Warning,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Diagnostic {
    pub r#type: DiagnosticType,
    pub severity: Severity,
    pub message: String,
    pub field: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CellStatus {
    pub valid: bool,
    pub diagnostics: Vec<Diagnostic>,
}

/// Compute diagnostics that depend only on the cell's own content.
///
/// Currently this checks:
/// - Whether the cell's diff (if present) applies cleanly → `DiffConflict`
///
/// The result is determined entirely by the cell's source and metadata, so it
/// is safe to cache keyed by the cell's SHA.
pub fn compute_own_diagnostics(cell: &nbformat::v4::Cell) -> Vec<Diagnostic> {
    let mut diagnostics: Vec<Diagnostic> = Vec::new();
    if let Some(data) = cell.ipso() {
        if let Some(diff) = &data.diff {
            let source = cell.source_str();
            if apply_diff(&source, diff).is_err() {
                diagnostics.push(Diagnostic {
                    r#type: DiagnosticType::DiffConflict,
                    severity: Severity::Error,
                    message: "diff does not apply cleanly to the current source. Regenerate the diff or update the source to match.".to_string(),
                    field: "diff".to_string(),
                });
            }
        }
    }
    diagnostics
}

/// Compute diagnostics that depend on the cell's position within the notebook
/// (cross-cell state checks).
///
/// This calls `cell_state` and maps:
/// - `CellState::Missing` → `Missing` error
/// - `CellState::Changed` → `NeedsReview` / `AncestorModified` warnings
///
/// Must always be called fresh — never cached — because the result depends on
/// other cells.
pub fn compute_state_diagnostics(
    nb: &nbformat::v4::Notebook,
    cell_index: usize,
) -> Vec<Diagnostic> {
    let mut diagnostics: Vec<Diagnostic> = Vec::new();
    match cell_state(nb, cell_index) {
        CellState::Valid => {}
        CellState::Missing => {
            diagnostics.push(Diagnostic {
                r#type: DiagnosticType::Missing,
                severity: Severity::Error,
                message: "This cell has no tests or fixtures. Use the `repair_ipso` MCP tool for guided setup, or run `ipso update` to add fixtures and a test, then `ipso accept` to baseline it.".to_string(),
                field: "shas".to_string(),
            });
        }
        CellState::Changed(result) => {
            for reason in result.needs_review {
                diagnostics.push(Diagnostic {
                    r#type: DiagnosticType::NeedsReview,
                    severity: Severity::Warning,
                    message: reason,
                    field: "shas".to_string(),
                });
            }
            for reason in result.ancestor_modified {
                diagnostics.push(Diagnostic {
                    r#type: DiagnosticType::AncestorModified,
                    severity: Severity::Warning,
                    message: reason,
                    field: "shas".to_string(),
                });
            }
        }
    }
    diagnostics
}

/// Compute diagnostics for the cell at `cell_index` in `nb`.
///
/// - `CellState::Missing` → `missing` error (code cell never configured or never accepted)
/// - `CellState::Changed` → `needs_review` warnings for own-content changes,
///   `ancestor_modified` warnings for preceding-cell changes
/// - If cell has a diff, checks whether it applies cleanly; if not → `diff_conflict` error
pub fn compute_cell_diagnostics(nb: &nbformat::v4::Notebook, cell_index: usize) -> CellStatus {
    let cell = &nb.cells[cell_index];
    let mut diagnostics = compute_state_diagnostics(nb, cell_index);
    diagnostics.extend(compute_own_diagnostics(cell));
    let valid = diagnostics.is_empty();
    CellStatus { valid, diagnostics }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::notebook::blank_cell_metadata;
    use crate::shas::compute_cell_sha;
    use nbformat::v4::{Cell, CellId, Metadata, Notebook};
    use serde_json::json;

    fn cid(s: &str) -> CellId {
        CellId::new(s).unwrap()
    }

    fn plain_cell(id: &str, source: &str) -> Cell {
        Cell::Code {
            id: cid(id),
            metadata: blank_cell_metadata(),
            execution_count: None,
            source: vec![source.to_string()],
            outputs: vec![],
        }
    }

    fn cell_with_shas(id: &str, source: &str, shas: serde_json::Value) -> Cell {
        let mut meta = blank_cell_metadata();
        meta.additional
            .insert("ipso".to_string(), json!({ "shas": shas }));
        Cell::Code {
            id: cid(id),
            metadata: meta,
            execution_count: None,
            source: vec![source.to_string()],
            outputs: vec![],
        }
    }

    fn cell_with_nb_no_shas(id: &str, source: &str) -> Cell {
        let mut meta = blank_cell_metadata();
        meta.additional.insert("ipso".to_string(), json!({}));
        Cell::Code {
            id: cid(id),
            metadata: meta,
            execution_count: None,
            source: vec![source.to_string()],
            outputs: vec![],
        }
    }

    fn notebook(cells: Vec<Cell>) -> Notebook {
        Notebook {
            metadata: Metadata::default(),
            nbformat: 4,
            nbformat_minor: 5,
            cells,
        }
    }

    fn sha_json(cell: &Cell) -> serde_json::Value {
        json!({ "cell_id": cell.cell_id_str(), "sha": compute_cell_sha(cell) })
    }

    // ---------------------------------------------------------------------------
    // Helpers
    // ---------------------------------------------------------------------------

    fn cell_with_diff_and_shas(
        id: &str,
        source: &str,
        diff: &str,
        shas: serde_json::Value,
    ) -> Cell {
        let mut meta = blank_cell_metadata();
        meta.additional
            .insert("ipso".to_string(), json!({ "shas": shas, "diff": diff }));
        Cell::Code {
            id: cid(id),
            metadata: meta,
            execution_count: None,
            source: vec![source.to_string()],
            outputs: vec![],
        }
    }

    fn cell_with_diff_no_shas(id: &str, source: &str, diff: &str) -> Cell {
        let mut meta = blank_cell_metadata();
        meta.additional
            .insert("ipso".to_string(), json!({ "diff": diff }));
        Cell::Code {
            id: cid(id),
            metadata: meta,
            execution_count: None,
            source: vec![source.to_string()],
            outputs: vec![],
        }
    }

    fn markdown_cell(id: &str, source: &str) -> Cell {
        Cell::Markdown {
            id: cid(id),
            metadata: blank_cell_metadata(),
            source: vec![source.to_string()],
            attachments: None,
        }
    }

    // ---------------------------------------------------------------------------
    // DiagnosticType::Display
    // ---------------------------------------------------------------------------

    #[test]
    fn diagnostic_type_display_strings() {
        assert_eq!(DiagnosticType::Missing.to_string(), "missing");
        assert_eq!(DiagnosticType::NeedsReview.to_string(), "needs_review");
        assert_eq!(
            DiagnosticType::AncestorModified.to_string(),
            "ancestor_modified"
        );
        assert_eq!(DiagnosticType::DiffConflict.to_string(), "diff_conflict");
        assert_eq!(DiagnosticType::InvalidField.to_string(), "invalid_field");
    }

    // ---------------------------------------------------------------------------
    // field values
    // ---------------------------------------------------------------------------

    #[test]
    fn missing_diagnostic_has_shas_field() {
        let nb = notebook(vec![plain_cell("c1", "x = 1")]);
        let status = compute_cell_diagnostics(&nb, 0);
        assert_eq!(status.diagnostics[0].field, "shas");
    }

    #[test]
    fn needs_review_diagnostic_has_shas_field() {
        let c1_old = plain_cell("c1", "x = 1");
        let shas = json!([sha_json(&c1_old)]);
        let c1 = cell_with_shas("c1", "x = 999", shas);
        let nb = notebook(vec![c1]);
        let status = compute_cell_diagnostics(&nb, 0);
        let d = status
            .diagnostics
            .iter()
            .find(|d| d.r#type == DiagnosticType::NeedsReview)
            .unwrap();
        assert_eq!(d.field, "shas");
    }

    #[test]
    fn ancestor_modified_diagnostic_has_shas_field() {
        let c1_old = plain_cell("c1", "x = 1");
        let c2_plain = plain_cell("c2", "y = 2");
        let shas = json!([sha_json(&c1_old), sha_json(&c2_plain)]);
        let c1 = plain_cell("c1", "x = 999");
        let c2 = cell_with_shas("c2", "y = 2", shas);
        let nb = notebook(vec![c1, c2]);
        let status = compute_cell_diagnostics(&nb, 1);
        let d = status
            .diagnostics
            .iter()
            .find(|d| d.r#type == DiagnosticType::AncestorModified)
            .unwrap();
        assert_eq!(d.field, "shas");
    }

    #[test]
    fn diff_conflict_diagnostic_has_diff_field() {
        let diff = "--- a\n+++ b\n@@ -1 +1 @@\n-old line\n+new line\n".to_string();
        let c1_plain = plain_cell("c1", "completely different");
        let shas = json!([sha_json(&c1_plain)]);
        let cell = cell_with_diff_and_shas("c1", "completely different", &diff, shas);
        let nb = notebook(vec![cell]);
        let status = compute_cell_diagnostics(&nb, 0);
        let d = status
            .diagnostics
            .iter()
            .find(|d| d.r#type == DiagnosticType::DiffConflict)
            .unwrap();
        assert_eq!(d.field, "diff");
    }

    // ---------------------------------------------------------------------------
    // Markdown / raw cells
    // ---------------------------------------------------------------------------

    #[test]
    fn markdown_cell_produces_no_diagnostics() {
        let nb = notebook(vec![markdown_cell("m1", "# Hello")]);
        let status = compute_cell_diagnostics(&nb, 0);
        assert!(status.valid);
        assert!(status.diagnostics.is_empty());
    }

    // ---------------------------------------------------------------------------
    // Valid cell with a clean diff — no DiffConflict
    // ---------------------------------------------------------------------------

    #[test]
    fn valid_cell_with_clean_diff_produces_no_diagnostics() {
        // In ipso the cell stores the original source; the diff is applied
        // forward to get the patched version. apply_diff(original, diff) must succeed.
        let original = "x = 1\n";
        let patched = "x = 99\n";
        let diff = crate::diff_utils::compute_diff(original, patched).unwrap();

        // Build the cell with the original source and the diff, then compute its SHA.
        let cell_for_sha = {
            let mut m = blank_cell_metadata();
            m.additional
                .insert("ipso".to_string(), json!({ "diff": diff }));
            Cell::Code {
                id: cid("c1"),
                metadata: m,
                execution_count: None,
                source: vec![original.to_string()],
                outputs: vec![],
            }
        };
        let shas = json!([sha_json(&cell_for_sha)]);
        let cell = cell_with_diff_and_shas("c1", original, &diff, shas);
        let nb = notebook(vec![cell]);
        let status = compute_cell_diagnostics(&nb, 0);
        assert!(
            status.valid,
            "unexpected diagnostics: {:?}",
            status.diagnostics
        );
    }

    // ---------------------------------------------------------------------------
    // Combinations
    // ---------------------------------------------------------------------------

    #[test]
    fn needs_review_and_ancestor_modified_both_emitted() {
        // Both this cell's own source changed AND a predecessor changed.
        let c1_old = plain_cell("c1", "x = 1");
        let c2_old = plain_cell("c2", "y = 2");
        let shas = json!([sha_json(&c1_old), sha_json(&c2_old)]);
        let c1 = plain_cell("c1", "x = 999"); // ancestor changed
        let c2 = cell_with_shas("c2", "y = 999", shas); // own source changed too
        let nb = notebook(vec![c1, c2]);
        let status = compute_cell_diagnostics(&nb, 1);
        assert!(!status.valid);
        assert!(status
            .diagnostics
            .iter()
            .any(|d| d.r#type == DiagnosticType::NeedsReview));
        assert!(status
            .diagnostics
            .iter()
            .any(|d| d.r#type == DiagnosticType::AncestorModified));
    }

    #[test]
    fn needs_review_and_diff_conflict_both_emitted() {
        // Cell's own source changed (NeedsReview) AND the stored diff no longer applies.
        let bad_diff = "--- a\n+++ b\n@@ -1 +1 @@\n-old line\n+new line\n".to_string();
        // Record shas when source was "original source"
        let c1_for_sha = cell_with_diff_and_shas("c1", "original source", &bad_diff, json!([]));
        let shas = json!([sha_json(&c1_for_sha)]);
        // Now source is "completely different" — both sha mismatch and diff won't apply
        let cell = cell_with_diff_and_shas("c1", "completely different", &bad_diff, shas);
        let nb = notebook(vec![cell]);
        let status = compute_cell_diagnostics(&nb, 0);
        assert!(!status.valid);
        assert!(status
            .diagnostics
            .iter()
            .any(|d| d.r#type == DiagnosticType::NeedsReview));
        assert!(status
            .diagnostics
            .iter()
            .any(|d| d.r#type == DiagnosticType::DiffConflict));
    }

    #[test]
    fn ancestor_modified_and_diff_conflict_both_emitted() {
        // Ancestor changed (AncestorModified) AND the diff on this cell won't apply.
        let bad_diff = "--- a\n+++ b\n@@ -1 +1 @@\n-old line\n+new line\n".to_string();
        let c1_old = plain_cell("c1", "x = 1");
        let c2_for_sha =
            cell_with_diff_and_shas("c2", "completely different", &bad_diff, json!([]));
        let shas = json!([sha_json(&c1_old), sha_json(&c2_for_sha)]);
        let c1 = plain_cell("c1", "x = 999"); // ancestor changed
        let c2 = cell_with_diff_and_shas("c2", "completely different", &bad_diff, shas); // own sha unchanged
        let nb = notebook(vec![c1, c2]);
        let status = compute_cell_diagnostics(&nb, 1);
        assert!(!status.valid);
        assert!(status
            .diagnostics
            .iter()
            .any(|d| d.r#type == DiagnosticType::AncestorModified));
        assert!(status
            .diagnostics
            .iter()
            .any(|d| d.r#type == DiagnosticType::DiffConflict));
        // Own source didn't change, so no NeedsReview
        assert!(!status
            .diagnostics
            .iter()
            .any(|d| d.r#type == DiagnosticType::NeedsReview));
    }

    #[test]
    fn missing_and_diff_conflict_both_emitted_when_nb_has_diff_but_no_shas() {
        // Cell has ipso with a diff but no shas — Missing fires.
        // The diff also doesn't apply — DiffConflict fires too.
        let bad_diff = "--- a\n+++ b\n@@ -1 +1 @@\n-old line\n+new line\n".to_string();
        let cell = cell_with_diff_no_shas("c1", "completely different", &bad_diff);
        let nb = notebook(vec![cell]);
        let status = compute_cell_diagnostics(&nb, 0);
        assert!(!status.valid);
        assert!(status
            .diagnostics
            .iter()
            .any(|d| d.r#type == DiagnosticType::Missing));
        assert!(status
            .diagnostics
            .iter()
            .any(|d| d.r#type == DiagnosticType::DiffConflict));
    }

    #[test]
    fn multiple_ancestor_reasons_each_produce_a_diagnostic() {
        // Both a deletion and a modification among ancestors → two AncestorModified diagnostics.
        let c1_old = plain_cell("c1", "x = 1");
        let c_gone = plain_cell("c-gone", "old");
        let c2_plain = plain_cell("c2", "y = 2");
        let shas = json!([sha_json(&c1_old), sha_json(&c_gone), sha_json(&c2_plain)]);
        // c1 was also modified on top of c_gone being deleted
        let c1 = plain_cell("c1", "x = 999");
        let c2 = cell_with_shas("c2", "y = 2", shas);
        let nb = notebook(vec![c1, c2]);
        let status = compute_cell_diagnostics(&nb, 1);
        let ancestor_diags: Vec<_> = status
            .diagnostics
            .iter()
            .filter(|d| d.r#type == DiagnosticType::AncestorModified)
            .collect();
        assert!(
            ancestor_diags.len() >= 2,
            "expected at least 2 AncestorModified diagnostics (deleted + modified), got {}",
            ancestor_diags.len()
        );
    }

    #[test]
    fn all_three_state_plus_diff_conflict() {
        // NeedsReview + AncestorModified + DiffConflict all at once.
        let bad_diff = "--- a\n+++ b\n@@ -1 +1 @@\n-old line\n+new line\n".to_string();
        let c1_old = plain_cell("c1", "x = 1");
        let c2_for_sha = cell_with_diff_and_shas("c2", "original", &bad_diff, json!([]));
        let shas = json!([sha_json(&c1_old), sha_json(&c2_for_sha)]);
        let c1 = plain_cell("c1", "x = 999"); // ancestor changed
        let c2 = cell_with_diff_and_shas("c2", "completely different", &bad_diff, shas); // own source changed + bad diff
        let nb = notebook(vec![c1, c2]);
        let status = compute_cell_diagnostics(&nb, 1);
        assert!(!status.valid);
        assert!(status
            .diagnostics
            .iter()
            .any(|d| d.r#type == DiagnosticType::NeedsReview));
        assert!(status
            .diagnostics
            .iter()
            .any(|d| d.r#type == DiagnosticType::AncestorModified));
        assert!(status
            .diagnostics
            .iter()
            .any(|d| d.r#type == DiagnosticType::DiffConflict));
    }

    #[test]
    fn valid_cell_no_diagnostics() {
        let c1_plain = plain_cell("c1", "x = 1");
        let shas = json!([sha_json(&c1_plain)]);
        let c1 = cell_with_shas("c1", "x = 1", shas);
        let nb = notebook(vec![c1]);
        let status = compute_cell_diagnostics(&nb, 0);
        assert!(status.valid);
        assert!(status.diagnostics.is_empty());
    }

    #[test]
    fn plain_code_cell_produces_missing_error() {
        let nb = notebook(vec![plain_cell("c1", "x = 1")]);
        let status = compute_cell_diagnostics(&nb, 0);
        assert!(!status.valid);
        assert_eq!(status.diagnostics.len(), 1);
        assert_eq!(status.diagnostics[0].r#type, DiagnosticType::Missing);
        assert_eq!(status.diagnostics[0].severity, Severity::Error);
    }

    #[test]
    fn cell_with_nb_no_shas_produces_missing_error() {
        let nb = notebook(vec![cell_with_nb_no_shas("c1", "x = 1")]);
        let status = compute_cell_diagnostics(&nb, 0);
        assert!(!status.valid);
        assert_eq!(status.diagnostics.len(), 1);
        assert_eq!(status.diagnostics[0].r#type, DiagnosticType::Missing);
        assert_eq!(status.diagnostics[0].severity, Severity::Error);
    }

    #[test]
    fn changed_own_source_produces_needs_review_warning() {
        let c1_old = plain_cell("c1", "x = 1");
        let shas = json!([sha_json(&c1_old)]);
        // Cell is now "x = 999"
        let c1 = cell_with_shas("c1", "x = 999", shas);
        let nb = notebook(vec![c1]);
        let status = compute_cell_diagnostics(&nb, 0);
        assert!(!status.valid);
        assert!(status
            .diagnostics
            .iter()
            .any(|d| d.r#type == DiagnosticType::NeedsReview && d.severity == Severity::Warning));
    }

    #[test]
    fn ancestor_change_produces_ancestor_modified_warning() {
        let c1_old = plain_cell("c1", "x = 1");
        let c2_plain = plain_cell("c2", "y = 2");
        let shas = json!([sha_json(&c1_old), sha_json(&c2_plain)]);
        let c1 = plain_cell("c1", "x = 999"); // c1 changed
        let c2 = cell_with_shas("c2", "y = 2", shas); // c2 unchanged
        let nb = notebook(vec![c1, c2]);
        let status = compute_cell_diagnostics(&nb, 1);
        assert!(!status.valid);
        assert!(status.diagnostics.iter().any(
            |d| d.r#type == DiagnosticType::AncestorModified && d.severity == Severity::Warning
        ));
        assert!(!status
            .diagnostics
            .iter()
            .any(|d| d.r#type == DiagnosticType::NeedsReview));
    }

    #[test]
    fn diff_conflict_produces_error() {
        let diff = "--- a\n+++ b\n@@ -1 +1 @@\n-old line\n+new line\n".to_string();
        let mut meta = blank_cell_metadata();
        // Valid shas so no cell-state issues, but diff that won't apply
        let c1_plain = plain_cell("c1", "completely different");
        let shas = json!([sha_json(&c1_plain)]);
        meta.additional
            .insert("ipso".to_string(), json!({ "shas": shas, "diff": diff }));
        let cell = Cell::Code {
            id: cid("c1"),
            metadata: meta,
            execution_count: None,
            source: vec!["completely different".to_string()],
            outputs: vec![],
        };
        let nb = notebook(vec![cell]);
        let status = compute_cell_diagnostics(&nb, 0);
        assert!(status
            .diagnostics
            .iter()
            .any(|d| d.r#type == DiagnosticType::DiffConflict));
    }

    // ---------------------------------------------------------------------------
    // compute_own_diagnostics / compute_state_diagnostics
    // ---------------------------------------------------------------------------

    #[test]
    fn own_diagnostics_diff_conflict() {
        let diff = "--- a\n+++ b\n@@ -1 +1 @@\n-old line\n+new line\n".to_string();
        let cell = cell_with_diff_no_shas("c1", "completely different", &diff);
        let diags = compute_own_diagnostics(&cell);
        assert!(diags
            .iter()
            .any(|d| d.r#type == DiagnosticType::DiffConflict));
    }

    #[test]
    fn own_diagnostics_clean_diff() {
        let original = "x = 1\n";
        let patched = "x = 99\n";
        let diff = crate::diff_utils::compute_diff(original, patched).unwrap();
        let mut meta = blank_cell_metadata();
        meta.additional
            .insert("ipso".to_string(), json!({ "diff": diff }));
        let cell = Cell::Code {
            id: cid("c1"),
            metadata: meta,
            execution_count: None,
            source: vec![original.to_string()],
            outputs: vec![],
        };
        let diags = compute_own_diagnostics(&cell);
        assert!(diags.is_empty());
    }

    #[test]
    fn own_diagnostics_no_metadata() {
        let cell = plain_cell("c1", "x = 1");
        let diags = compute_own_diagnostics(&cell);
        assert!(diags.is_empty());
    }

    #[test]
    fn state_diagnostics_missing() {
        let nb = notebook(vec![plain_cell("c1", "x = 1")]);
        let diags = compute_state_diagnostics(&nb, 0);
        assert!(diags.iter().any(|d| d.r#type == DiagnosticType::Missing));
    }

    #[test]
    fn state_diagnostics_valid() {
        let c1_plain = plain_cell("c1", "x = 1");
        let shas = json!([sha_json(&c1_plain)]);
        let c1 = cell_with_shas("c1", "x = 1", shas);
        let nb = notebook(vec![c1]);
        let diags = compute_state_diagnostics(&nb, 0);
        assert!(diags.is_empty());
    }

    #[test]
    fn state_diagnostics_both_buckets() {
        let c1_old = plain_cell("c1", "x = 1");
        let c2_old = plain_cell("c2", "y = 2");
        let shas = json!([sha_json(&c1_old), sha_json(&c2_old)]);
        let c1 = plain_cell("c1", "x = 999");
        let c2 = cell_with_shas("c2", "y = 999", shas);
        let nb = notebook(vec![c1, c2]);
        let diags = compute_state_diagnostics(&nb, 1);
        assert!(diags
            .iter()
            .any(|d| d.r#type == DiagnosticType::NeedsReview));
        assert!(diags
            .iter()
            .any(|d| d.r#type == DiagnosticType::AncestorModified));
    }
}
