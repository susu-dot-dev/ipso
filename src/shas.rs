use nbformat::v4::{Cell, Notebook};
use serde_json::Value;
use sha1::{Digest, Sha1};
use std::collections::HashSet;

use crate::metadata::{NotaBeneData, ShaEntry};
use crate::notebook::CellExt;

// ---------------------------------------------------------------------------
// compute_cell_sha
// ---------------------------------------------------------------------------

/// Compute a deterministic SHA1 for a cell based on its source content.
/// Uses canonical JSON of the source string for stability.
pub fn compute_cell_sha(cell: &Cell) -> String {
    let source = cell.source_str();
    // canonical_json requires a serde_json::Value
    let val = Value::String(source);
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

// ---------------------------------------------------------------------------
// Staleness
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq)]
pub enum Staleness {
    /// Cell has been reviewed and its shas match current notebook state.
    Valid,
    /// Cell has nota-bene metadata but no shas snapshot.
    NotImplemented,
    /// Cell shas exist but don't match current notebook state.
    OutOfDate(Vec<String>),
}

/// Compute staleness for a single code cell at position `cell_index` in `nb`.
///
/// `cell_index` is the 0-based index of the cell in `nb.cells`.
pub fn staleness(nb: &Notebook, cell_index: usize) -> Staleness {
    let cell = &nb.cells[cell_index];

    let data: NotaBeneData = match cell.nota_bene() {
        None => return Staleness::Valid, // no metadata → not our concern
        Some(d) => d,
    };

    let stored_shas = match &data.shas {
        None => return Staleness::NotImplemented,
        Some(s) => s,
    };

    // Build current snapshot up to and including this cell (all preceding cells
    // affect whether this cell's tests are still valid).
    let current_snapshot = compute_snapshot(nb);

    let mut reasons = Vec::new();

    // Check ordering: stored cell IDs must appear in the same order.
    let stored_ids: Vec<&str> = stored_shas.iter().map(|s| s.cell_id.as_str()).collect();
    let current_ids: Vec<&str> = current_snapshot
        .iter()
        .map(|s| s.cell_id.as_str())
        .collect();
    let stored_id_set: HashSet<&str> = stored_ids.iter().copied().collect();
    let current_id_set: HashSet<&str> = current_ids.iter().copied().collect();

    // Check for deleted cells (in stored but not in current).
    for entry in stored_shas.iter() {
        if !current_id_set.contains(entry.cell_id.as_str()) {
            reasons.push(format!(
                "Preceding cell `{}` was deleted (present at last validation, now missing)",
                entry.cell_id
            ));
        }
    }

    // Check for inserted cells (in current but not in stored), only for cells
    // preceding the target cell.
    let target_id = cell.cell_id_str();
    let target_pos_current = current_ids.iter().position(|&id| id == target_id);
    if let Some(target_pos) = target_pos_current {
        for id in &current_ids[..target_pos] {
            if !stored_id_set.contains(id) {
                reasons.push(format!(
                    "Preceding cell `{}` was inserted (not present at last validation)",
                    id
                ));
            }
        }
    }

    // Check ordering change.
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
        reasons.push("Cell ordering changed since last validation".to_string());
    }

    // Check SHA of this cell itself.
    let current_sha = compute_cell_sha(cell);
    if let Some(stored) = stored_shas.iter().find(|e| e.cell_id == target_id) {
        if stored.sha != current_sha {
            reasons.push("Cell source changed since fixtures were last validated".to_string());
        }
    }

    // Check SHAs of preceding cells.
    if let Some(target_pos) = target_pos_current {
        for current_entry in &current_snapshot[..target_pos] {
            if let Some(stored_entry) = stored_shas
                .iter()
                .find(|e| e.cell_id == current_entry.cell_id)
            {
                if stored_entry.sha != current_entry.sha {
                    reasons.push(format!(
                        "Preceding cell `{}` was modified",
                        current_entry.cell_id
                    ));
                }
            }
        }
    }

    if reasons.is_empty() {
        Staleness::Valid
    } else {
        Staleness::OutOfDate(reasons)
    }
}

// ---------------------------------------------------------------------------
// accept_cell
// ---------------------------------------------------------------------------

/// Recompute and store the SHA snapshot for a single cell at `cell_index`.
/// Only stamps the cell if it has nota-bene metadata.
pub fn accept_cell(nb: &mut Notebook, cell_index: usize) {
    let snapshot = compute_snapshot(nb);
    let cell = &mut nb.cells[cell_index];
    if cell.nota_bene().is_some() {
        let shas_slice = snapshot[..=cell_index].to_vec();
        cell.nota_bene_mut().set_shas(shas_slice);
    }
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

    /// Build a cell that has nota-bene metadata with a shas snapshot.
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

    /// Build a cell with present nota-bene metadata but no shas key.
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

    // --- staleness ---

    #[test]
    fn absent_meta_returns_valid() {
        let nb = notebook(vec![plain_cell("c1", "x = 1")]);
        assert_eq!(staleness(&nb, 0), Staleness::Valid);
    }

    #[test]
    fn no_shas_field_returns_not_implemented() {
        let nb = notebook(vec![cell_with_nb_no_shas("c1", "x = 1")]);
        assert_eq!(staleness(&nb, 0), Staleness::NotImplemented);
    }

    #[test]
    fn matching_shas_returns_valid() {
        let c1 = plain_cell("c1", "x = 1");
        let c2_plain = plain_cell("c2", "y = 2");
        let shas = json!([sha_json(&c1), sha_json(&c2_plain)]);
        let c2 = cell_with_shas("c2", "y = 2", shas);
        let nb = notebook(vec![c1, c2]);
        assert_eq!(staleness(&nb, 1), Staleness::Valid);
    }

    #[test]
    fn single_cell_with_its_own_sha_valid() {
        let c_plain = plain_cell("c1", "x = 1");
        let shas = json!([sha_json(&c_plain)]);
        let c = cell_with_shas("c1", "x = 1", shas);
        let nb = notebook(vec![c]);
        assert_eq!(staleness(&nb, 0), Staleness::Valid);
    }

    #[test]
    fn target_cell_source_changed_returns_out_of_date() {
        let c1 = plain_cell("c1", "x = 1");
        let c2_old = plain_cell("c2", "y = 2");
        let shas = json!([sha_json(&c1), sha_json(&c2_old)]);
        // Cell is now "y = 999" instead of "y = 2"
        let c2 = cell_with_shas("c2", "y = 999", shas);
        let nb = notebook(vec![c1, c2]);
        match staleness(&nb, 1) {
            Staleness::OutOfDate(reasons) => {
                assert!(reasons.iter().any(|r| r.contains("Cell source changed")));
            }
            other => panic!("expected OutOfDate, got {other:?}"),
        }
    }

    #[test]
    fn preceding_cell_modified_returns_out_of_date() {
        let c1_old = plain_cell("c1", "x = 1");
        let c2_plain = plain_cell("c2", "y = 2");
        let shas = json!([sha_json(&c1_old), sha_json(&c2_plain)]);
        // c1 is now "x = 999"
        let c1 = plain_cell("c1", "x = 999");
        let c2 = cell_with_shas("c2", "y = 2", shas);
        let nb = notebook(vec![c1, c2]);
        match staleness(&nb, 1) {
            Staleness::OutOfDate(reasons) => {
                assert!(reasons.iter().any(|r| r.contains("modified")));
            }
            other => panic!("expected OutOfDate, got {other:?}"),
        }
    }

    #[test]
    fn cell_deleted_returns_out_of_date() {
        let c1 = plain_cell("c1", "x = 1");
        let c_gone = plain_cell("c-gone", "old");
        let c2_plain = plain_cell("c2", "y = 2");
        let shas = json!([sha_json(&c1), sha_json(&c_gone), sha_json(&c2_plain)]);
        // c-gone is no longer in the notebook
        let c2 = cell_with_shas("c2", "y = 2", shas);
        let nb = notebook(vec![c1, c2]);
        match staleness(&nb, 1) {
            Staleness::OutOfDate(reasons) => {
                assert!(reasons.iter().any(|r| r.contains("deleted")));
            }
            other => panic!("expected OutOfDate, got {other:?}"),
        }
    }

    #[test]
    fn cell_inserted_before_target_returns_out_of_date() {
        let c1 = plain_cell("c1", "x = 1");
        let c2_plain = plain_cell("c2", "y = 2");
        let shas = json!([sha_json(&c1), sha_json(&c2_plain)]);
        // A new cell was inserted between c1 and c2.
        let c_new = plain_cell("c-new", "new stuff");
        let c2 = cell_with_shas("c2", "y = 2", shas);
        let nb = notebook(vec![c1, c_new, c2]);
        match staleness(&nb, 2) {
            Staleness::OutOfDate(reasons) => {
                assert!(reasons.iter().any(|r| r.contains("inserted")));
            }
            other => panic!("expected OutOfDate, got {other:?}"),
        }
    }

    #[test]
    fn cell_reordered_returns_out_of_date() {
        let c1 = plain_cell("c1", "x = 1");
        let c2_plain = plain_cell("c2", "y = 2");
        // Stored order: [c1, c2]; now the notebook has [c2, c1].
        let shas = json!([sha_json(&c1), sha_json(&c2_plain)]);
        let c2 = cell_with_shas("c2", "y = 2", shas);
        let nb = notebook(vec![c2, c1]); // reordered
        match staleness(&nb, 0) {
            Staleness::OutOfDate(reasons) => {
                assert!(reasons.iter().any(|r| r.contains("ordering")));
            }
            other => panic!("expected OutOfDate, got {other:?}"),
        }
    }

    #[test]
    fn inserted_after_target_does_not_trigger_out_of_date() {
        // A new cell inserted AFTER the target should not make it stale.
        let c1_plain = plain_cell("c1", "x = 1");
        let shas = json!([sha_json(&c1_plain)]);
        let c1 = cell_with_shas("c1", "x = 1", shas);
        let c_after = plain_cell("c-after", "new");
        let nb = notebook(vec![c1, c_after]);
        // c-after is after c1, so it shouldn't affect staleness of c1.
        assert_eq!(staleness(&nb, 0), Staleness::Valid);
    }

    // --- accept_cell ---

    #[test]
    fn accept_cell_stamps_nb_cell() {
        let c1 = plain_cell("c1", "x = 1");
        let c2 = cell_with_nb_no_shas("c2", "y = 2");
        let mut nb = notebook(vec![c1, c2]);
        accept_cell(&mut nb, 1);
        let data = nb.cells[1].nota_bene().expect("c2 should have nota-bene");
        let shas = data.shas.expect("shas should be set");
        assert_eq!(shas.len(), 2); // snapshot up to and including c2
        assert_eq!(shas[1].cell_id, "c2");
    }

    #[test]
    fn accept_cell_skips_cell_without_nb_metadata() {
        let c1 = plain_cell("c1", "x = 1"); // no nota-bene
        let mut nb = notebook(vec![c1]);
        accept_cell(&mut nb, 0);
        assert!(nb.cells[0].nota_bene().is_none());
    }

    #[test]
    fn accept_cell_reaccept_overwrites_shas() {
        let c1 = plain_cell("c1", "x = 1");
        let c2 = cell_with_nb_no_shas("c2", "y = 2");
        let mut nb = notebook(vec![c1, c2]);
        accept_cell(&mut nb, 1);
        let shas_before = nb.cells[1].nota_bene().unwrap().shas.unwrap();

        // Modify c1's source, then re-accept c2 — shas should change
        nb.cells[0] = plain_cell("c1", "x = 999");
        accept_cell(&mut nb, 1);
        let shas_after = nb.cells[1].nota_bene().unwrap().shas.unwrap();

        assert_ne!(
            shas_before[0].sha, shas_after[0].sha,
            "shas should reflect new c1 source"
        );
    }

    #[test]
    fn accept_cell_produces_valid_staleness() {
        let c1 = plain_cell("c1", "x = 1");
        let c2 = cell_with_nb_no_shas("c2", "y = 2");
        let mut nb = notebook(vec![c1, c2]);
        accept_cell(&mut nb, 1);
        assert_eq!(staleness(&nb, 1), Staleness::Valid);
    }
}
