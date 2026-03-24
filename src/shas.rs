use nbformat::v4::{Cell, Notebook};
use serde_json::Value;
use sha1::{Digest, Sha1};
use std::collections::HashSet;

use crate::metadata::{IpsoData, ShaEntry};
use crate::notebook::CellExt;

/// SHA1 of `{"ipso": <metadata without "shas">, "source": "..."}`.
/// `shas` is excluded to avoid a circular dependency.
pub fn compute_cell_sha(cell: &Cell) -> String {
    let source = cell.source_str();

    let ipso_val: Value = match cell.additional().get("ipso") {
        Some(Value::Object(map)) => {
            let filtered: serde_json::Map<String, Value> = map
                .iter()
                .filter(|(k, _)| k.as_str() != "shas")
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect();
            Value::Object(filtered)
        }
        _ => Value::Object(serde_json::Map::new()),
    };

    let val = serde_json::json!({
        "ipso": ipso_val,
        "source": source,
    });

    let canonical = canonical_json::to_string(&val).unwrap_or_default();
    let mut hasher = Sha1::new();
    hasher.update(canonical.as_bytes());
    format!("{:x}", hasher.finalize())
}

/// Compute a snapshot of ShaEntries for all cells in the notebook.
pub fn compute_snapshot(nb: &Notebook) -> Vec<ShaEntry> {
    nb.cells
        .iter()
        .map(|cell| ShaEntry {
            cell_id: cell.cell_id_str().to_string(),
            sha: compute_cell_sha(cell),
        })
        .collect()
}

/// Reasons why a cell's own content is suspect (the cell itself was modified).
/// Anything derived from the cell's `shas` array that reflects cross-cell
/// comparisons (ancestor checks) lives in `ancestor_modified` instead.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct CellStateResult {
    /// Reasons the cell's own SHA no longer matches stored state.
    pub needs_review: Vec<String>,
    /// Reasons a preceding cell changed, was deleted, inserted, or reordered.
    pub ancestor_modified: Vec<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum CellState {
    /// SHAs match; everything checks out.
    Valid,
    /// Code cell with no ipso setup, or ipso present but no shas recorded yet.
    Missing,
    /// SHAs exist but something has changed since last accept.
    Changed(CellStateResult),
}

/// Compute the cell state for a single cell at position `cell_index` in `nb`.
///
/// - Code cells with no `ipso` metadata return `Missing` (bug fix: they
///   must not be silently ignored — the AI needs to set them up).
/// - Cells with `ipso` but no `shas` return `Missing`.
/// - Cells whose SHAs have drifted return `Changed` with reasons split into
///   `needs_review` (own content changed) and `ancestor_modified` (preceding
///   cell changes).
pub fn cell_state(nb: &Notebook, cell_index: usize) -> CellState {
    let cell = &nb.cells[cell_index];

    let data: IpsoData = match cell.ipso() {
        None => {
            // Code cells with no ipso metadata have never been configured.
            if matches!(cell, Cell::Code { .. }) {
                return CellState::Missing;
            }
            return CellState::Valid;
        }
        Some(d) => d,
    };

    let stored_shas = match &data.shas {
        None => return CellState::Missing,
        Some(s) => s,
    };

    let current_snapshot = compute_snapshot(nb);

    let mut result = CellStateResult::default();

    let stored_ids: Vec<&str> = stored_shas.iter().map(|s| s.cell_id.as_str()).collect();
    let current_ids: Vec<&str> = current_snapshot
        .iter()
        .map(|s| s.cell_id.as_str())
        .collect();
    let stored_id_set: HashSet<&str> = stored_ids.iter().copied().collect();
    let current_id_set: HashSet<&str> = current_ids.iter().copied().collect();

    for entry in stored_shas.iter() {
        if !current_id_set.contains(entry.cell_id.as_str()) {
            result.ancestor_modified.push(format!(
                "Cell `{}` was deleted from the notebook after this cell was last accepted. Re-run `nb accept` to update the baseline.",
                entry.cell_id
            ));
        }
    }

    let target_id = cell.cell_id_str();
    let target_pos_current = current_ids.iter().position(|&id| id == target_id);
    if let Some(target_pos) = target_pos_current {
        for id in &current_ids[..target_pos] {
            if !stored_id_set.contains(id) {
                result.ancestor_modified.push(format!(
                    "Cell `{}` was inserted before this cell after it was last accepted. Re-run `nb accept` to update the baseline.",
                    id
                ));
            }
        }
    }

    // Filter to cells present in both snapshots, then compare sequence.
    let stored_existing: Vec<&str> = stored_ids
        .iter()
        .copied()
        .filter(|id| current_id_set.contains(id))
        .collect();
    let current_existing: Vec<&str> = current_ids
        .iter()
        .copied()
        .filter(|id| stored_id_set.contains(id))
        .collect();
    if stored_existing != current_existing {
        result
            .ancestor_modified
            .push("Cells were reordered since this cell was last accepted. Re-run `nb accept` to update the baseline.".to_string());
    }

    let current_sha = compute_cell_sha(cell);
    if let Some(stored) = stored_shas.iter().find(|e| e.cell_id == target_id) {
        if stored.sha != current_sha {
            result
                .needs_review
                .push("This cell's source or metadata changed since it was last accepted. Review the changes and run `nb accept` to re-baseline.".to_string());
        }
    }

    if let Some(target_pos) = target_pos_current {
        for current_entry in &current_snapshot[..target_pos] {
            if let Some(stored_entry) = stored_shas
                .iter()
                .find(|e| e.cell_id == current_entry.cell_id)
            {
                if stored_entry.sha != current_entry.sha {
                    result.ancestor_modified.push(format!(
                        "Cell `{}` (which this cell depends on) was modified since this cell was last accepted. Check whether the changes affect this cell's tests, then run `nb accept`.",
                        current_entry.cell_id
                    ));
                }
            }
        }
    }

    if result.needs_review.is_empty() && result.ancestor_modified.is_empty() {
        CellState::Valid
    } else {
        CellState::Changed(result)
    }
}

/// Store the SHA snapshot for a cell.
///
/// - If the cell already has `ipso` metadata, shas are updated in place.
/// - If the cell is a plain code cell with no `ipso` metadata, a minimal
///   `ipso` structure is created (empty fixtures/diff/test) so the cell
///   is acknowledged and becomes Valid after acceptance.
/// - Non-code cells (Markdown, Raw) are skipped.
pub fn accept_cell(nb: &mut Notebook, cell_index: usize) {
    let snapshot = compute_snapshot(nb);
    let cell = &mut nb.cells[cell_index];
    if !matches!(cell, Cell::Code { .. }) {
        return;
    }
    // Ensure ipso key exists (creates it for plain code cells).
    cell.ipso_mut().mark_addressed();
    let shas_slice = snapshot[..=cell_index].to_vec();
    cell.ipso_mut().set_shas(shas_slice);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::notebook::blank_cell_metadata;
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

    // --- compute_cell_sha ---

    #[test]
    fn sha_is_deterministic() {
        let c = plain_cell("c1", "x = 1");
        assert_eq!(compute_cell_sha(&c), compute_cell_sha(&c));
    }

    #[test]
    fn sha_changes_when_source_changes() {
        let c1 = plain_cell("c1", "x = 1");
        let c2 = plain_cell("c1", "x = 2");
        assert_ne!(compute_cell_sha(&c1), compute_cell_sha(&c2));
    }

    #[test]
    fn sha_does_not_depend_on_cell_id() {
        let c1 = plain_cell("id-a", "x = 1");
        let c2 = plain_cell("id-b", "x = 1");
        assert_eq!(compute_cell_sha(&c1), compute_cell_sha(&c2));
    }

    // --- compute_snapshot ---

    #[test]
    fn snapshot_empty_notebook() {
        let snap = compute_snapshot(&notebook(vec![]));
        assert!(snap.is_empty());
    }

    #[test]
    fn snapshot_has_entry_per_cell_in_order() {
        let nb = notebook(vec![plain_cell("c1", "x = 1"), plain_cell("c2", "y = 2")]);
        let snap = compute_snapshot(&nb);
        assert_eq!(snap.len(), 2);
        assert_eq!(snap[0].cell_id, "c1");
        assert_eq!(snap[1].cell_id, "c2");
    }

    #[test]
    fn snapshot_sha_matches_compute_cell_sha() {
        let c = plain_cell("c1", "x = 1");
        let expected = compute_cell_sha(&c);
        let nb = notebook(vec![c]);
        assert_eq!(compute_snapshot(&nb)[0].sha, expected);
    }

    fn markdown_cell(id: &str, source: &str) -> Cell {
        Cell::Markdown {
            id: cid(id),
            metadata: blank_cell_metadata(),
            source: vec![source.to_string()],
            attachments: None,
        }
    }

    // --- cell_state ---

    #[test]
    fn markdown_cell_no_meta_returns_valid() {
        // Markdown cells are not under ipso management; they must never
        // produce a Missing state, regardless of whether they have metadata.
        let nb = notebook(vec![markdown_cell("m1", "# Hello")]);
        assert_eq!(cell_state(&nb, 0), CellState::Valid);
    }

    #[test]
    fn plain_code_cell_no_meta_returns_missing() {
        let nb = notebook(vec![plain_cell("c1", "x = 1")]);
        assert_eq!(cell_state(&nb, 0), CellState::Missing);
    }

    #[test]
    fn no_shas_field_returns_missing() {
        let nb = notebook(vec![cell_with_nb_no_shas("c1", "x = 1")]);
        assert_eq!(cell_state(&nb, 0), CellState::Missing);
    }

    #[test]
    fn matching_shas_returns_valid() {
        let c1 = plain_cell("c1", "x = 1");
        let c2_plain = plain_cell("c2", "y = 2");
        let shas = json!([sha_json(&c1), sha_json(&c2_plain)]);
        let c2 = cell_with_shas("c2", "y = 2", shas);
        let nb = notebook(vec![c1, c2]);
        assert_eq!(cell_state(&nb, 1), CellState::Valid);
    }

    #[test]
    fn single_cell_with_its_own_sha_valid() {
        let c_plain = plain_cell("c1", "x = 1");
        let shas = json!([sha_json(&c_plain)]);
        let c = cell_with_shas("c1", "x = 1", shas);
        let nb = notebook(vec![c]);
        assert_eq!(cell_state(&nb, 0), CellState::Valid);
    }

    #[test]
    fn target_cell_source_changed_returns_needs_review() {
        let c1 = plain_cell("c1", "x = 1");
        let c2_old = plain_cell("c2", "y = 2");
        let shas = json!([sha_json(&c1), sha_json(&c2_old)]);
        let c2 = cell_with_shas("c2", "y = 999", shas);
        let nb = notebook(vec![c1, c2]);
        match cell_state(&nb, 1) {
            CellState::Changed(result) => {
                assert!(result.needs_review.iter().any(|r| r.contains("changed")));
                assert!(result.ancestor_modified.is_empty());
            }
            other => panic!("expected Changed, got {other:?}"),
        }
    }

    #[test]
    fn preceding_cell_modified_returns_ancestor_modified() {
        let c1_old = plain_cell("c1", "x = 1");
        let c2_plain = plain_cell("c2", "y = 2");
        let shas = json!([sha_json(&c1_old), sha_json(&c2_plain)]);
        let c1 = plain_cell("c1", "x = 999");
        let c2 = cell_with_shas("c2", "y = 2", shas);
        let nb = notebook(vec![c1, c2]);
        match cell_state(&nb, 1) {
            CellState::Changed(result) => {
                assert!(result
                    .ancestor_modified
                    .iter()
                    .any(|r| r.contains("modified")));
                assert!(result.needs_review.is_empty());
            }
            other => panic!("expected Changed, got {other:?}"),
        }
    }

    #[test]
    fn cell_deleted_returns_ancestor_modified() {
        let c1 = plain_cell("c1", "x = 1");
        let c_gone = plain_cell("c-gone", "old");
        let c2_plain = plain_cell("c2", "y = 2");
        let shas = json!([sha_json(&c1), sha_json(&c_gone), sha_json(&c2_plain)]);
        let c2 = cell_with_shas("c2", "y = 2", shas);
        let nb = notebook(vec![c1, c2]);
        match cell_state(&nb, 1) {
            CellState::Changed(result) => {
                assert!(result
                    .ancestor_modified
                    .iter()
                    .any(|r| r.contains("deleted")));
            }
            other => panic!("expected Changed, got {other:?}"),
        }
    }

    #[test]
    fn cell_inserted_before_target_returns_ancestor_modified() {
        let c1 = plain_cell("c1", "x = 1");
        let c2_plain = plain_cell("c2", "y = 2");
        let shas = json!([sha_json(&c1), sha_json(&c2_plain)]);
        let c_new = plain_cell("c-new", "new stuff");
        let c2 = cell_with_shas("c2", "y = 2", shas);
        let nb = notebook(vec![c1, c_new, c2]);
        match cell_state(&nb, 2) {
            CellState::Changed(result) => {
                assert!(result
                    .ancestor_modified
                    .iter()
                    .any(|r| r.contains("inserted")));
            }
            other => panic!("expected Changed, got {other:?}"),
        }
    }

    #[test]
    fn cell_reordered_returns_ancestor_modified() {
        let c1 = plain_cell("c1", "x = 1");
        let c2_plain = plain_cell("c2", "y = 2");
        let shas = json!([sha_json(&c1), sha_json(&c2_plain)]);
        let c2 = cell_with_shas("c2", "y = 2", shas);
        let nb = notebook(vec![c2, c1]); // reordered
        match cell_state(&nb, 0) {
            CellState::Changed(result) => {
                assert!(result
                    .ancestor_modified
                    .iter()
                    .any(|r| r.contains("reorder") || r.contains("reorder")));
            }
            other => panic!("expected Changed, got {other:?}"),
        }
    }

    #[test]
    fn both_self_and_ancestor_changed_populates_both_buckets() {
        let c1_old = plain_cell("c1", "x = 1");
        let c2_old = plain_cell("c2", "y = 2");
        let shas = json!([sha_json(&c1_old), sha_json(&c2_old)]);
        // c1 changed (ancestor) AND c2's own source changed
        let c1 = plain_cell("c1", "x = 999");
        let c2 = cell_with_shas("c2", "y = 999", shas);
        let nb = notebook(vec![c1, c2]);
        match cell_state(&nb, 1) {
            CellState::Changed(result) => {
                assert!(
                    !result.needs_review.is_empty(),
                    "needs_review should be non-empty"
                );
                assert!(
                    !result.ancestor_modified.is_empty(),
                    "ancestor_modified should be non-empty"
                );
            }
            other => panic!("expected Changed, got {other:?}"),
        }
    }

    #[test]
    fn inserted_after_target_does_not_trigger_changed() {
        let c1_plain = plain_cell("c1", "x = 1");
        let shas = json!([sha_json(&c1_plain)]);
        let c1 = cell_with_shas("c1", "x = 1", shas);
        let c_after = plain_cell("c-after", "new");
        let nb = notebook(vec![c1, c_after]);
        // c-after is after c1, so it shouldn't affect cell_state of c1.
        assert_eq!(cell_state(&nb, 0), CellState::Valid);
    }

    // --- compute_cell_sha metadata sensitivity ---

    fn cell_with_fixture(id: &str, source: &str, fixture_val: &str) -> Cell {
        let mut meta = blank_cell_metadata();
        meta.additional.insert(
            "ipso".to_string(),
            json!({ "fixtures": { "input": fixture_val } }),
        );
        Cell::Code {
            id: cid(id),
            metadata: meta,
            execution_count: None,
            source: vec![source.to_string()],
            outputs: vec![],
        }
    }

    fn cell_with_diff(id: &str, source: &str, diff_val: &str) -> Cell {
        let mut meta = blank_cell_metadata();
        meta.additional
            .insert("ipso".to_string(), json!({ "diff": diff_val }));
        Cell::Code {
            id: cid(id),
            metadata: meta,
            execution_count: None,
            source: vec![source.to_string()],
            outputs: vec![],
        }
    }

    fn cell_with_test(id: &str, source: &str, test_val: &str) -> Cell {
        let mut meta = blank_cell_metadata();
        meta.additional
            .insert("ipso".to_string(), json!({ "test": { "code": test_val } }));
        Cell::Code {
            id: cid(id),
            metadata: meta,
            execution_count: None,
            source: vec![source.to_string()],
            outputs: vec![],
        }
    }

    fn cell_with_shas_only(id: &str, source: &str) -> Cell {
        let mut meta = blank_cell_metadata();
        meta.additional.insert(
            "ipso".to_string(),
            json!({ "shas": [{"cell_id": id, "sha": "deadbeef"}] }),
        );
        Cell::Code {
            id: cid(id),
            metadata: meta,
            execution_count: None,
            source: vec![source.to_string()],
            outputs: vec![],
        }
    }

    #[test]
    fn sha_changes_when_fixture_changes() {
        let c1 = cell_with_fixture("c1", "x = 1", "old_fixture");
        let c2 = cell_with_fixture("c1", "x = 1", "new_fixture");
        assert_ne!(
            compute_cell_sha(&c1),
            compute_cell_sha(&c2),
            "SHA must change when fixture metadata changes"
        );
    }

    #[test]
    fn sha_changes_when_diff_changes() {
        let c1 = cell_with_diff("c1", "x = 1", "old diff");
        let c2 = cell_with_diff("c1", "x = 1", "new diff");
        assert_ne!(
            compute_cell_sha(&c1),
            compute_cell_sha(&c2),
            "SHA must change when diff metadata changes"
        );
    }

    #[test]
    fn sha_changes_when_test_changes() {
        let c1 = cell_with_test("c1", "x = 1", "assert x == 1");
        let c2 = cell_with_test("c1", "x = 1", "assert x == 2");
        assert_ne!(
            compute_cell_sha(&c1),
            compute_cell_sha(&c2),
            "SHA must change when test metadata changes"
        );
    }

    #[test]
    fn sha_does_not_change_when_only_shas_change() {
        let c1 = cell_with_shas_only("c1", "x = 1");
        let mut meta = blank_cell_metadata();
        meta.additional.insert(
            "ipso".to_string(),
            json!({ "shas": [{"cell_id": "c1", "sha": "different_sha_value"}] }),
        );
        let c2 = Cell::Code {
            id: cid("c1"),
            metadata: meta,
            execution_count: None,
            source: vec!["x = 1".to_string()],
            outputs: vec![],
        };
        assert_eq!(
            compute_cell_sha(&c1),
            compute_cell_sha(&c2),
            "SHA must NOT change when only shas key changes"
        );
    }

    #[test]
    fn sha_plain_cell_uses_empty_ipso_object() {
        let plain = plain_cell("c1", "x = 1");
        let with_empty_nb = cell_with_nb_no_shas("c1", "x = 1");
        assert_eq!(
            compute_cell_sha(&plain),
            compute_cell_sha(&with_empty_nb),
            "Plain cell and cell with empty ipso must hash identically"
        );
    }

    // --- accept_cell ---

    #[test]
    fn accept_cell_stamps_nb_cell() {
        let c1 = plain_cell("c1", "x = 1");
        let c2 = cell_with_nb_no_shas("c2", "y = 2");
        let mut nb = notebook(vec![c1, c2]);
        accept_cell(&mut nb, 1);
        let data = nb.cells[1].ipso().expect("c2 should have ipso");
        let shas = data.shas.expect("shas should be set");
        assert_eq!(shas.len(), 2);
        assert_eq!(shas[1].cell_id, "c2");
    }

    #[test]
    fn accept_cell_creates_nb_metadata_on_plain_code_cell() {
        let c1 = plain_cell("c1", "x = 1");
        let mut nb = notebook(vec![c1]);
        accept_cell(&mut nb, 0);
        let data = nb.cells[0].ipso().expect("ipso should exist after accept");
        let shas = data.shas.expect("shas should be set");
        assert_eq!(shas.len(), 1);
        assert_eq!(shas[0].cell_id, "c1");
        assert_eq!(cell_state(&nb, 0), CellState::Valid);
    }

    #[test]
    fn accept_cell_reaccept_overwrites_shas() {
        let c1 = plain_cell("c1", "x = 1");
        let c2 = cell_with_nb_no_shas("c2", "y = 2");
        let mut nb = notebook(vec![c1, c2]);
        accept_cell(&mut nb, 1);
        let shas_before = nb.cells[1].ipso().unwrap().shas.unwrap();

        // Modify c1's source, then re-accept c2 — shas should change
        nb.cells[0] = plain_cell("c1", "x = 999");
        accept_cell(&mut nb, 1);
        let shas_after = nb.cells[1].ipso().unwrap().shas.unwrap();

        assert_ne!(
            shas_before[0].sha, shas_after[0].sha,
            "shas should reflect new c1 source"
        );
    }

    #[test]
    fn accept_cell_produces_valid_state() {
        let c1 = plain_cell("c1", "x = 1");
        let c2 = cell_with_nb_no_shas("c2", "y = 2");
        let mut nb = notebook(vec![c1, c2]);
        accept_cell(&mut nb, 1);
        assert_eq!(cell_state(&nb, 1), CellState::Valid);
    }
}
