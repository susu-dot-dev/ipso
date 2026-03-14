use serde::{Deserialize, Serialize};

use crate::diff_utils::apply_diff;
use crate::notebook::CellExt;
use crate::shas::staleness;

// ---------------------------------------------------------------------------
// Diagnostic types
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum DiagnosticType {
    MissingSha,
    Stale,
    DiffConflict,
    MissingField,
    InvalidValue,
    UnknownCell,
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

// ---------------------------------------------------------------------------
// compute_cell_diagnostics
// ---------------------------------------------------------------------------

/// Compute diagnostics for the cell at `cell_index` in `nb`.
///
/// - `Staleness::NotImplemented` → `missing_sha` warning
/// - `Staleness::OutOfDate(reasons)` → one `stale` error per reason
/// - If cell has a diff, checks whether it applies cleanly; if not → `diff_conflict` error
pub fn compute_cell_diagnostics(nb: &nbformat::v4::Notebook, cell_index: usize) -> CellStatus {
    let mut diagnostics: Vec<Diagnostic> = Vec::new();
    let cell = &nb.cells[cell_index];

    // Staleness checks.
    match staleness(nb, cell_index) {
        crate::shas::Staleness::Valid => {}
        crate::shas::Staleness::NotImplemented => {
            diagnostics.push(Diagnostic {
                r#type: DiagnosticType::MissingSha,
                severity: Severity::Warning,
                message: "cell has never been accepted".to_string(),
                field: "shas".to_string(),
            });
        }
        crate::shas::Staleness::OutOfDate(reasons) => {
            for reason in reasons {
                diagnostics.push(Diagnostic {
                    r#type: DiagnosticType::Stale,
                    severity: Severity::Error,
                    message: reason,
                    field: "shas".to_string(),
                });
            }
        }
    }

    // Diff conflict check.
    if let Some(data) = cell.nota_bene() {
        if let Some(diff) = &data.diff {
            let source = cell.source_str();
            if apply_diff(&source, diff).is_err() {
                diagnostics.push(Diagnostic {
                    r#type: DiagnosticType::DiffConflict,
                    severity: Severity::Error,
                    message: "diff does not apply cleanly to current cell source".to_string(),
                    field: "diff".to_string(),
                });
            }
        }
    }

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
            .insert("nota-bene".to_string(), json!({ "shas": shas }));
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
        meta.additional.insert("nota-bene".to_string(), json!({}));
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
    fn missing_sha_produces_warning() {
        let nb = notebook(vec![cell_with_nb_no_shas("c1", "x = 1")]);
        let status = compute_cell_diagnostics(&nb, 0);
        assert!(!status.valid);
        assert_eq!(status.diagnostics.len(), 1);
        assert_eq!(status.diagnostics[0].r#type, DiagnosticType::MissingSha);
        assert_eq!(status.diagnostics[0].severity, Severity::Warning);
    }

    #[test]
    fn stale_cell_produces_error() {
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
            .any(|d| d.r#type == DiagnosticType::Stale));
    }

    #[test]
    fn diff_conflict_produces_error() {
        let diff = "--- a\n+++ b\n@@ -1 +1 @@\n-old line\n+new line\n".to_string();
        let mut meta = blank_cell_metadata();
        // Valid shas so no staleness, but diff that won't apply to "completely different"
        let c1_plain = plain_cell("c1", "completely different");
        let shas = json!([sha_json(&c1_plain)]);
        meta.additional.insert(
            "nota-bene".to_string(),
            json!({ "shas": shas, "diff": diff }),
        );
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

    #[test]
    fn no_meta_cell_is_valid() {
        let nb = notebook(vec![plain_cell("c1", "x = 1")]);
        let status = compute_cell_diagnostics(&nb, 0);
        assert!(status.valid);
    }
}
