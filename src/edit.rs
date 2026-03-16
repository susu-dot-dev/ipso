use anyhow::Result;
use nbformat::v4::{Cell, Notebook};
use serde_json::json;

use crate::diff_utils::{apply_diff, reconstruct_original};
use crate::metadata::{Fixture, NotaBeneData};
use crate::notebook::{blank_cell_metadata, new_cell_id, CellExt};
use crate::shas::{cell_state, compute_snapshot, CellState};

// ---------------------------------------------------------------------------
// Public entry point
// ---------------------------------------------------------------------------

/// Convert a source notebook into a test-editor notebook.
///
/// `source_path` is stored in the editor notebook metadata so `--continue` can
/// locate the original file; pass the path as a string (relative or absolute).
pub fn build_editor_notebook(source: &Notebook, source_path: &str) -> Result<Notebook> {
    let mut cells: Vec<Cell> = Vec::new();
    let source_path_str = source_path;

    // 1. Setup cell
    cells.push(make_setup_cell());

    // 2. One section per source cell, in order
    for (idx, cell) in source.cells.iter().enumerate() {
        // 1-based position counting all cells
        let cell_pos = idx + 1;

        match cell {
            Cell::Code { .. } => {
                let nb_meta = cell.nota_bene();
                match &nb_meta {
                    None => {
                        cells.push(make_section_header_no_tests(cell, cell_pos));
                        cells.push(make_source_cell_passthrough(cell));
                    }
                    Some(data) => {
                        let state = cell_state(source, idx);
                        cells.push(make_section_header_with_meta(cell, cell_pos, data, &state)?);

                        // Fixture cells — or a stub if none exist
                        if let Some(fixtures) = &data.fixtures {
                            let mut sorted: Vec<(&String, &Fixture)> = fixtures.iter().collect();
                            sorted.sort_by_key(|(_, f)| f.priority);
                            for (name, fixture) in sorted {
                                cells.push(make_fixture_cell(cell.cell_id_str(), name, fixture));
                            }
                        } else {
                            // No real fixtures — emit a stub so the user has a cell to fill in.
                            cells.push(make_stub_fixture_cell(cell.cell_id_str()));
                        }

                        // Patched source cell (borrows the source cell's Jupyter ID)
                        let patched_source = match &data.diff {
                            Some(diff) => apply_diff(&cell.source_str(), diff)
                                .unwrap_or_else(|_| cell.source_str()),
                            None => cell.source_str(),
                        };
                        cells.push(make_patched_source_cell(
                            cell.cell_id_str(),
                            &patched_source,
                        ));

                        // Test cell
                        let (test_name, test_src) = match &data.test {
                            Some(t) => (t.name.clone(), t.source.clone()),
                            None => ("<unnamed>".to_string(), String::new()),
                        };
                        cells.push(make_test_cell(cell.cell_id_str(), &test_name, &test_src));
                    }
                }
            }
            // Non-code cells are skipped in the editor notebook
            Cell::Markdown { .. } | Cell::Raw { .. } => {}
        }
    }

    let mut nb = Notebook {
        metadata: source.metadata.clone(),
        nbformat: 4,
        nbformat_minor: 5,
        cells,
    };

    // Clear any nota-bene notebook-level metadata from the editor notebook,
    // then store the source SHA snapshot for conflict detection in --continue.
    nb.metadata.additional.remove("nota-bene");
    let source_shas = compute_snapshot(source);
    nb.metadata.additional.insert(
        "nota-bene".to_string(),
        json!({
            "editor": {
                "source_path": source_path_str,
                "source_shas": source_shas
            }
        }),
    );

    Ok(nb)
}

// ---------------------------------------------------------------------------
// Cell constructors
// ---------------------------------------------------------------------------

const SETUP_SOURCE: &str = "import nota_bene\nnota_bene.register_nb_skip()";

fn make_setup_cell() -> Cell {
    let mut meta = blank_cell_metadata();
    meta.additional.insert(
        "nota-bene".to_string(),
        json!({"editor": {"role": "setup"}}),
    );
    Cell::Code {
        id: new_cell_id(),
        metadata: meta,
        execution_count: None,
        source: split_source(SETUP_SOURCE),
        outputs: vec![],
    }
}

fn make_section_header_no_tests(cell: &Cell, pos: usize) -> Cell {
    let cell_id = cell.cell_id_str();
    let source = format!(
        "---\n## `{cell_id}` · Cell {pos} · No tests\n\nNo tests yet. Edit the source cell below, write a test with `%%nb_skip` / `# test: <name>`, then run `nota-bene edit --continue <notebook>`.",
        pos = pos,
        cell_id = cell_id
    );
    let mut meta = blank_cell_metadata();
    meta.additional.insert(
        "nota-bene".to_string(),
        json!({"editor": {"role": "section-header", "cell_id": cell_id}}),
    );
    Cell::Markdown {
        id: new_cell_id(),
        metadata: meta,
        source: split_source(&source),
        attachments: None,
    }
}

fn make_section_header_with_meta(
    cell: &Cell,
    pos: usize,
    data: &NotaBeneData,
    state: &CellState,
) -> Result<Cell> {
    let cell_id = cell.cell_id_str();

    let (status_str, reasons) = match state {
        CellState::Valid => ("Has tests", vec![]),
        CellState::Missing => (
            "Needs review",
            vec!["This cell has no tests or fixtures yet. Add them in the cells below, then run `nota-bene edit --continue` to save.".to_string()],
        ),
        CellState::Changed(result) => {
            let mut all_reasons = result.needs_review.clone();
            all_reasons.extend(result.ancestor_modified.clone());
            ("Needs review", all_reasons)
        }
    };

    let action_hint = match state {
        CellState::Valid => "Tests are up to date. Edit the source or test cells below, then run `nota-bene edit --continue <notebook>`.",
        CellState::Missing => "Tests exist but have never been validated against the current source (no SHA recorded yet). Review the test cell below, then run `nota-bene edit --continue <notebook>` to lock in the current state.",
        CellState::Changed(_) => "The source has changed since these tests were saved. Review and update the test cell below, then run `nota-bene edit --continue <notebook>`.",
    };

    let mut parts = vec![format!(
        "---\n## `{cell_id}` · Cell {pos} · {status_str}\n\n{action_hint}",
        pos = pos,
        cell_id = cell_id,
        status_str = status_str,
        action_hint = action_hint,
    )];

    if !reasons.is_empty() {
        parts.push(format!("**Why:** {}", reasons.join("; ")));
    }

    if let Some(diff) = &data.diff {
        let original =
            reconstruct_original(&cell.source_str(), diff).unwrap_or_else(|_| cell.source_str());
        parts.push(format!(
            "**Original source** *(before patch — read only)*\n\n```python\n{}\n```",
            original
        ));
    }

    let source = parts.join("\n\n");
    let mut meta = blank_cell_metadata();
    meta.additional.insert(
        "nota-bene".to_string(),
        json!({"editor": {"role": "section-header", "cell_id": cell_id}}),
    );
    Ok(Cell::Markdown {
        id: new_cell_id(),
        metadata: meta,
        source: split_source(&source),
        attachments: None,
    })
}

fn make_source_cell_passthrough(cell: &Cell) -> Cell {
    // For cells without nota-bene metadata, emit the original source, carrying
    // the original Jupyter cell ID.
    if let Cell::Code {
        id,
        source,
        execution_count,
        outputs,
        ..
    } = cell
    {
        let mut meta = blank_cell_metadata();
        meta.additional.insert(
            "nota-bene".to_string(),
            json!({"editor": {"role": "source", "cell_id": id.as_str()}}),
        );
        Cell::Code {
            id: id.clone(),
            metadata: meta,
            execution_count: *execution_count,
            source: source.clone(),
            outputs: outputs.clone(),
        }
    } else {
        cell.clone()
    }
}

/// Stub fixture cell emitted when a `Present` cell has no real fixtures.
/// The body is intentionally empty — leaving it blank on `--continue` signals
/// "no fixtures needed" (`Some(None)`). Filling it in creates a real fixture.
fn make_stub_fixture_cell(parent_cell_id: &str) -> Cell {
    let source = "# fixture: \n# description: \n# priority: 0";
    let mut meta = blank_cell_metadata();
    meta.additional.insert(
        "nota-bene".to_string(),
        json!({"editor": {"role": "fixture", "cell_id": parent_cell_id, "fixture_name": ""}}),
    );
    Cell::Code {
        id: new_cell_id(),
        metadata: meta,
        execution_count: None,
        source: split_source(source),
        outputs: vec![],
    }
}

fn make_fixture_cell(parent_cell_id: &str, name: &str, fixture: &Fixture) -> Cell {
    let header = format!(
        "# fixture: {}\n# description: {}\n# priority: {}",
        name, fixture.description, fixture.priority
    );
    let source = if fixture.source.is_empty() {
        header
    } else {
        format!("{}\n{}", header, fixture.source)
    };
    let mut meta = blank_cell_metadata();
    meta.additional.insert(
        "nota-bene".to_string(),
        json!({"editor": {"role": "fixture", "cell_id": parent_cell_id, "fixture_name": name}}),
    );
    Cell::Code {
        id: new_cell_id(),
        metadata: meta,
        execution_count: None,
        source: split_source(&source),
        outputs: vec![],
    }
}

fn make_patched_source_cell(cell_id: &str, source: &str) -> Cell {
    let mut meta = blank_cell_metadata();
    meta.additional.insert(
        "nota-bene".to_string(),
        json!({"editor": {"role": "patched-source", "cell_id": cell_id}}),
    );
    Cell::Code {
        id: nbformat::v4::CellId::new(cell_id).unwrap_or_else(|_| new_cell_id()),
        metadata: meta,
        execution_count: None,
        source: split_source(source),
        outputs: vec![],
    }
}

fn make_test_cell(cell_id: &str, test_name: &str, test_source: &str) -> Cell {
    let source = if test_source.is_empty() {
        format!("%%nb_skip\n# test: {}", test_name)
    } else {
        format!("%%nb_skip\n# test: {}\n{}", test_name, test_source)
    };
    let mut meta = blank_cell_metadata();
    meta.additional.insert(
        "nota-bene".to_string(),
        json!({"editor": {"role": "test", "cell_id": cell_id}}),
    );
    Cell::Code {
        id: new_cell_id(),
        metadata: meta,
        execution_count: None,
        source: split_source(&source),
        outputs: vec![],
    }
}

// ---------------------------------------------------------------------------
// Helper: split source string into Vec<String> lines
// ---------------------------------------------------------------------------

pub fn split_source(s: &str) -> Vec<String> {
    if s.is_empty() {
        return vec![];
    }
    s.split_inclusive('\n').map(|l| l.to_string()).collect()
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

    fn code_cell(id: &str, source: &str) -> Cell {
        Cell::Code {
            id: cid(id),
            metadata: blank_cell_metadata(),
            execution_count: None,
            source: split_source(source),
            outputs: vec![],
        }
    }

    fn md_cell(id: &str, source: &str) -> Cell {
        Cell::Markdown {
            id: cid(id),
            metadata: blank_cell_metadata(),
            source: split_source(source),
            attachments: None,
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

    // --- split_source ---

    #[test]
    fn split_source_empty_returns_empty_vec() {
        assert_eq!(split_source(""), Vec::<String>::new());
    }

    #[test]
    fn split_source_single_line_no_newline() {
        assert_eq!(split_source("hello"), vec!["hello"]);
    }

    #[test]
    fn split_source_preserves_trailing_newlines_on_all_but_last() {
        assert_eq!(split_source("a\nb\n"), vec!["a\n", "b\n"]);
    }

    #[test]
    fn split_source_last_line_has_no_trailing_newline() {
        assert_eq!(split_source("a\nb"), vec!["a\n", "b"]);
    }

    #[test]
    fn split_source_single_newline_only() {
        assert_eq!(split_source("\n"), vec!["\n"]);
    }

    // --- build_editor_notebook structure ---

    #[test]
    fn first_cell_is_setup() {
        let nb = notebook(vec![code_cell("c1", "x = 1")]);
        let editor = build_editor_notebook(&nb, "test.ipynb").unwrap();
        assert_eq!(editor.cells[0].editor_role().as_deref(), Some("setup"));
    }

    #[test]
    fn setup_cell_has_correct_source() {
        let nb = notebook(vec![]);
        let editor = build_editor_notebook(&nb, "test.ipynb").unwrap();
        assert!(editor.cells[0]
            .source_str()
            .contains("nota_bene.register_nb_skip"));
    }

    #[test]
    fn markdown_cell_is_skipped_in_editor_notebook() {
        let nb = notebook(vec![md_cell("md-1", "# Title\n")]);
        let editor = build_editor_notebook(&nb, "test.ipynb").unwrap();
        // Non-code cells are skipped; only the setup cell should be present
        assert_eq!(editor.cells.len(), 1);
        assert_eq!(editor.cells[0].editor_role().as_deref(), Some("setup"));
    }

    #[test]
    fn code_cell_without_meta_produces_header_and_passthrough_source() {
        let nb = notebook(vec![code_cell("c1", "x = 1")]);
        let editor = build_editor_notebook(&nb, "test.ipynb").unwrap();
        // [setup, section-header, source]
        assert_eq!(editor.cells.len(), 3);
        assert_eq!(
            editor.cells[1].editor_role().as_deref(),
            Some("section-header")
        );
        assert_eq!(editor.cells[2].editor_role().as_deref(), Some("source"));
        assert_eq!(editor.cells[2].source_str(), "x = 1");
    }

    #[test]
    fn section_header_references_source_cell_id() {
        let nb = notebook(vec![code_cell("my-cell", "x = 1")]);
        let editor = build_editor_notebook(&nb, "test.ipynb").unwrap();
        assert_eq!(editor.cells[1].editor_cell_id().as_deref(), Some("my-cell"));
    }

    #[test]
    fn code_cell_with_meta_produces_header_source_and_test_cells() {
        let mut meta = blank_cell_metadata();
        meta.additional.insert("nota-bene".to_string(), json!({}));
        let cell = Cell::Code {
            id: cid("c1"),
            metadata: meta,
            execution_count: None,
            source: split_source("x = 1"),
            outputs: vec![],
        };
        let nb = notebook(vec![cell]);
        let editor = build_editor_notebook(&nb, "test.ipynb").unwrap();
        // [setup, section-header, stub-fixture, patched-source, test]
        assert_eq!(editor.cells.len(), 5);
        assert_eq!(editor.cells[4].editor_role().as_deref(), Some("test"));
    }

    #[test]
    fn fixture_cells_emitted_sorted_by_priority() {
        let mut meta = blank_cell_metadata();
        meta.additional.insert(
            "nota-bene".to_string(),
            json!({
                "fixtures": {
                    "z_fix": {"description": "d", "priority": 10, "source": "z = 10"},
                    "a_fix": {"description": "d", "priority": 1,  "source": "a = 1"}
                }
            }),
        );
        let cell = Cell::Code {
            id: cid("c1"),
            metadata: meta,
            execution_count: None,
            source: split_source("x = 1"),
            outputs: vec![],
        };
        let nb = notebook(vec![cell]);
        let editor = build_editor_notebook(&nb, "test.ipynb").unwrap();

        let fixture_cells: Vec<_> = editor
            .cells
            .iter()
            .filter(|c| c.editor_role().as_deref() == Some("fixture"))
            .collect();

        assert_eq!(fixture_cells.len(), 2);
        // Lower priority first.
        assert!(fixture_cells[0].source_str().contains("a_fix"));
        assert!(fixture_cells[1].source_str().contains("z_fix"));
    }

    #[test]
    fn editor_notebook_has_nota_bene_editor_with_source_shas() {
        let mut nb = notebook(vec![]);
        nb.metadata
            .additional
            .insert("nota-bene".to_string(), json!({"key": "val"}));
        let editor = build_editor_notebook(&nb, "test.ipynb").unwrap();
        // The editor notebook should have nota-bene.editor with source_shas.
        let nb_meta = editor.metadata.additional.get("nota-bene").unwrap();
        assert!(nb_meta.get("editor").is_some());
        assert!(nb_meta["editor"].get("source_shas").is_some());
        assert!(nb_meta["editor"].get("source_path").is_some());
        // Original keys from source notebook-level metadata should be cleared.
        assert!(nb_meta.get("key").is_none());
    }

    #[test]
    fn patched_source_cell_uses_diff_when_present() {
        let original = "x = 1\n";
        let patched = "x = 99\n";
        let diff = crate::diff_utils::compute_diff(original, patched).unwrap();
        let mut meta = blank_cell_metadata();
        meta.additional
            .insert("nota-bene".to_string(), json!({ "diff": diff }));
        let cell = Cell::Code {
            id: cid("c1"),
            metadata: meta,
            execution_count: None,
            source: split_source(original),
            outputs: vec![],
        };
        let nb = notebook(vec![cell]);
        let editor = build_editor_notebook(&nb, "test.ipynb").unwrap();

        let src_cell = editor
            .cells
            .iter()
            .find(|c| c.editor_role().as_deref() == Some("patched-source"))
            .unwrap();
        assert_eq!(src_cell.source_str(), patched);
    }

    #[test]
    fn test_cell_source_includes_nb_skip_and_name() {
        let mut meta = blank_cell_metadata();
        meta.additional.insert(
            "nota-bene".to_string(),
            json!({ "test": { "name": "check_x", "source": "assert x == 1" } }),
        );
        let cell = Cell::Code {
            id: cid("c1"),
            metadata: meta,
            execution_count: None,
            source: split_source("x = 1"),
            outputs: vec![],
        };
        let nb = notebook(vec![cell]);
        let editor = build_editor_notebook(&nb, "test.ipynb").unwrap();

        let test_cell = editor
            .cells
            .iter()
            .find(|c| c.editor_role().as_deref() == Some("test"))
            .unwrap();
        let src = test_cell.source_str();
        assert!(src.starts_with("%%nb_skip\n"));
        assert!(src.contains("# test: check_x"));
        assert!(src.contains("assert x == 1"));
    }

    // -------------------------------------------------------------------------
    // Stub fixture / test emission
    // -------------------------------------------------------------------------

    /// A `Present` cell with no fixtures should emit exactly one stub fixture cell.
    #[test]
    fn present_cell_with_no_fixtures_emits_stub_fixture() {
        let mut meta = blank_cell_metadata();
        meta.additional.insert("nota-bene".to_string(), json!({}));
        let cell = Cell::Code {
            id: cid("c1"),
            metadata: meta,
            execution_count: None,
            source: split_source("x = 1"),
            outputs: vec![],
        };
        let nb = notebook(vec![cell]);
        let editor = build_editor_notebook(&nb, "test.ipynb").unwrap();

        let fixture_cells: Vec<_> = editor
            .cells
            .iter()
            .filter(|c| c.editor_role().as_deref() == Some("fixture"))
            .collect();
        assert_eq!(
            fixture_cells.len(),
            1,
            "expected exactly one stub fixture cell"
        );

        // Stub body should be empty (only comment headers).
        let stub_src = fixture_cells[0].source_str();
        assert!(
            stub_src.contains("# fixture:"),
            "stub missing # fixture: header"
        );
        assert!(
            stub_src.contains("# priority: 0"),
            "stub missing # priority header"
        );
    }

    /// A `Present` cell with existing fixtures should NOT emit a stub — it emits
    /// the real fixture cells instead.
    #[test]
    fn present_cell_with_real_fixtures_emits_no_stub() {
        let mut meta = blank_cell_metadata();
        meta.additional.insert(
            "nota-bene".to_string(),
            json!({
                "fixtures": {
                    "my_fix": {"description": "d", "priority": 0, "source": "x = 1"}
                }
            }),
        );
        let cell = Cell::Code {
            id: cid("c1"),
            metadata: meta,
            execution_count: None,
            source: split_source("x = 1"),
            outputs: vec![],
        };
        let nb = notebook(vec![cell]);
        let editor = build_editor_notebook(&nb, "test.ipynb").unwrap();

        let fixture_cells: Vec<_> = editor
            .cells
            .iter()
            .filter(|c| c.editor_role().as_deref() == Some("fixture"))
            .collect();
        assert_eq!(fixture_cells.len(), 1);
        // The real fixture source must be present, not a stub.
        assert!(
            fixture_cells[0].source_str().contains("x = 1"),
            "expected real fixture source"
        );
    }

    /// A `Present` cell with `fixtures: null` (`Some(None)`) should also emit a
    /// stub so the user can decide whether to add one.
    #[test]
    fn present_cell_with_explicit_null_fixtures_emits_stub() {
        let mut meta = blank_cell_metadata();
        meta.additional
            .insert("nota-bene".to_string(), json!({"fixtures": null}));
        let cell = Cell::Code {
            id: cid("c1"),
            metadata: meta,
            execution_count: None,
            source: split_source("x = 1"),
            outputs: vec![],
        };
        let nb = notebook(vec![cell]);
        let editor = build_editor_notebook(&nb, "test.ipynb").unwrap();

        let fixture_cells: Vec<_> = editor
            .cells
            .iter()
            .filter(|c| c.editor_role().as_deref() == Some("fixture"))
            .collect();
        assert_eq!(
            fixture_cells.len(),
            1,
            "expected stub even when fixtures is explicit null"
        );
    }

    /// A Valid cell with `diff: null` (Some(None)) should NOT show "Original source".
    #[test]
    fn section_header_valid_with_explicit_null_diff_omits_original_source() {
        let mut meta = blank_cell_metadata();
        // shas must be present so cell_state() returns Valid, not Missing.
        // Use an empty shas list for simplicity (no preceding cells to check).
        meta.additional.insert(
            "nota-bene".to_string(),
            serde_json::json!({ "diff": null, "shas": [] }),
        );
        let cell = Cell::Code {
            id: cid("c1"),
            metadata: meta,
            execution_count: None,
            source: split_source("x = 1"),
            outputs: vec![],
        };
        let nb = notebook(vec![cell]);
        let editor = build_editor_notebook(&nb, "/some/path.ipynb").unwrap();

        let header = editor
            .cells
            .iter()
            .find(|c| c.editor_role().as_deref() == Some("section-header"))
            .expect("section header not found");

        let src = header.source_str();
        assert!(
            !src.contains("Original source"),
            "header should NOT contain 'Original source' when diff is explicit null, but got:\n{src}"
        );
    }

    /// A Valid cell with a real diff should show "Original source".
    #[test]
    fn section_header_valid_with_real_diff_includes_original_source() {
        let original = "x = 1\n";
        let patched = "x = 99\n";
        let diff = crate::diff_utils::compute_diff(original, patched).unwrap();
        let mut meta = blank_cell_metadata();
        meta.additional.insert(
            "nota-bene".to_string(),
            serde_json::json!({ "diff": diff, "shas": [] }),
        );
        let cell = Cell::Code {
            id: cid("c1"),
            metadata: meta,
            execution_count: None,
            source: split_source(patched),
            outputs: vec![],
        };
        let nb = notebook(vec![cell]);
        let editor = build_editor_notebook(&nb, "/some/path.ipynb").unwrap();

        let header = editor
            .cells
            .iter()
            .find(|c| c.editor_role().as_deref() == Some("section-header"))
            .expect("section header not found");

        let src = header.source_str();
        assert!(
            src.contains("Original source"),
            "header should contain 'Original source' when diff is present, but got:\n{src}"
        );
    }

    /// A changed cell with no diff should NOT include the original source section.
    #[test]
    fn section_header_out_of_date_without_diff_omits_original_source() {
        // Cell has nota-bene metadata with shas (triggering cell_state check),
        // but no diff. The section header should not contain "Original source".
        let c1_old = plain_cell_for_staleness("c1", "x = 1");
        let c2_meta = {
            let mut meta = blank_cell_metadata();
            // shas that record c1 as "x = 1" but c1 is now "x = 999" → Changed
            let shas = serde_json::json!([
                {"cell_id": "c1", "sha": crate::shas::compute_cell_sha(&c1_old)},
            ]);
            meta.additional
                .insert("nota-bene".to_string(), serde_json::json!({ "shas": shas }));
            meta
        };
        // Now c1 is different to trigger Changed
        let c1_changed = plain_cell_for_staleness("c1", "x = 999");
        let c2 = Cell::Code {
            id: cid("c2"),
            metadata: c2_meta,
            execution_count: None,
            source: split_source("y = 2"),
            outputs: vec![],
        };
        let nb = notebook(vec![c1_changed, c2]);
        let editor = build_editor_notebook(&nb, "/some/path.ipynb").unwrap();

        let header = editor
            .cells
            .iter()
            .find(|c| {
                c.editor_role().as_deref() == Some("section-header")
                    && c.editor_cell_id().as_deref() == Some("c2")
            })
            .expect("section header for c2 not found");

        let src = header.source_str();
        assert!(
            src.contains("Needs review"),
            "expected 'Needs review' in header but got:\n{src}"
        );
        assert!(
            !src.contains("Original source"),
            "header should NOT contain 'Original source' when there is no diff, but got:\n{src}"
        );
    }

    fn plain_cell_for_staleness(id: &str, source: &str) -> Cell {
        Cell::Code {
            id: cid(id),
            metadata: blank_cell_metadata(),
            execution_count: None,
            source: split_source(source),
            outputs: vec![],
        }
    }

    #[test]
    fn present_cell_with_no_test_emits_stub_test() {
        let mut meta = blank_cell_metadata();
        meta.additional.insert("nota-bene".to_string(), json!({}));
        let cell = Cell::Code {
            id: cid("c1"),
            metadata: meta,
            execution_count: None,
            source: split_source("x = 1"),
            outputs: vec![],
        };
        let nb = notebook(vec![cell]);
        let editor = build_editor_notebook(&nb, "test.ipynb").unwrap();

        let test_cells: Vec<_> = editor
            .cells
            .iter()
            .filter(|c| c.editor_role().as_deref() == Some("test"))
            .collect();
        assert_eq!(test_cells.len(), 1, "expected exactly one test cell");
        assert!(
            test_cells[0].source_str().starts_with("%%nb_skip\n"),
            "test stub must start with %%nb_skip"
        );
    }
}
