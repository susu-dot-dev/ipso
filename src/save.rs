use anyhow::{bail, Result};
use indexmap::IndexMap;
use nbformat::v4::{Cell, Notebook};
use std::collections::HashSet;

use crate::diff_utils::compute_diff;
use crate::metadata::{Fixture, ShaEntry, TestMeta};
use crate::notebook::{clear_editor_meta, CellExt};
use crate::shas::{compute_cell_sha, compute_snapshot};

// ---------------------------------------------------------------------------
// Section — parsed from the editor notebook
// ---------------------------------------------------------------------------

struct Section {
    cell_id: String,
    fixtures: Vec<(String, Fixture)>,
    patched_source: String,
    /// None means no test cell was found.
    test_name: Option<String>,
    test_source: Option<String>,
}

// ---------------------------------------------------------------------------
// Conflict detection
// ---------------------------------------------------------------------------

/// Read the stored `source_shas` from the editor notebook's notebook-level
/// metadata and compare against the current state of the source notebook.
///
/// Returns `Ok(())` if no conflicts are detected.
/// Returns `Err` with a diagnostic message if any cell has changed, been
/// reordered, inserted, or deleted since `edit` was run.
pub fn check_conflicts(source: &Notebook, editor: &Notebook) -> Result<()> {
    // Extract stored shas from editor notebook metadata.
    let stored_shas: Vec<ShaEntry> = editor
        .metadata
        .additional
        .get("nota-bene")
        .and_then(|v| v.get("editor"))
        .and_then(|v| v.get("source_shas"))
        .and_then(|v| serde_json::from_value(v.clone()).ok())
        .unwrap_or_default();

    if stored_shas.is_empty() {
        // No shas stored — editor notebook predates sha support; skip detection.
        return Ok(());
    }

    let stored_ids: Vec<&str> = stored_shas.iter().map(|e| e.cell_id.as_str()).collect();
    let current_ids: Vec<&str> = source.cells.iter().map(|c| c.cell_id_str()).collect();
    let stored_id_set: HashSet<&str> = stored_ids.iter().copied().collect();
    let current_id_set: HashSet<&str> = current_ids.iter().copied().collect();

    let mut errors: Vec<String> = Vec::new();

    // Check for deleted cells (in stored but not in current).
    for entry in &stored_shas {
        if !current_id_set.contains(entry.cell_id.as_str()) {
            errors.push(format!(
                "cell '{}' was deleted from the source notebook",
                entry.cell_id
            ));
        }
    }

    // Check for inserted cells (in current but not in stored).
    for id in &current_ids {
        if !stored_id_set.contains(id) {
            errors.push(format!(
                "cell '{}' was inserted into the source notebook",
                id
            ));
        }
    }

    // Check ordering (only among cells present in both).
    if errors.is_empty() {
        let current_order: Vec<&str> = current_ids
            .iter()
            .filter(|id| stored_id_set.contains(*id))
            .copied()
            .collect();
        let stored_order: Vec<&str> = stored_ids
            .iter()
            .filter(|id| current_id_set.contains(*id))
            .copied()
            .collect();
        if current_order != stored_order {
            errors.push("cell ordering changed in the source notebook".to_string());
        }
    }

    // Check content SHAs.
    if errors.is_empty() {
        for entry in &stored_shas {
            if let Some(cell) = source
                .cells
                .iter()
                .find(|c| c.cell_id_str() == entry.cell_id)
            {
                let current_sha = compute_cell_sha(cell);
                if current_sha != entry.sha {
                    errors.push(format!(
                        "cell '{}' source changed since `edit` was run",
                        entry.cell_id
                    ));
                }
            }
        }
    }

    if !errors.is_empty() {
        let msg = errors.join("\n  ");
        bail!(
            "Cannot apply: source notebook has changed since `edit` was run:\n  {msg}\n\n\
             Use `nota-bene edit --continue --force` to discard source changes and apply anyway."
        );
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Public entry point
// ---------------------------------------------------------------------------

/// Apply the changes from `editor` back onto `source` (in-place).
///
/// For every section successfully applied, the SHA snapshot is stamped on the
/// corresponding source cell — going through the editor IS the review.
pub fn apply_editor_to_source(source: &mut Notebook, editor: &Notebook) -> Result<()> {
    let sections = parse_sections(editor)?;

    // Compute snapshot once upfront. SHA is based on source content only (not
    // metadata), so it is stable across the metadata writes we are about to make.
    let snapshot = compute_snapshot(source);

    for section in &sections {
        // Find the matching cell in the source notebook by Jupyter cell ID.
        let result = source
            .cells
            .iter_mut()
            .enumerate()
            .find(|(_, c)| c.cell_id_str() == section.cell_id);

        let (cell_idx, src_cell) = match result {
            Some(pair) => pair,
            None => {
                eprintln!(
                    "warning: section cell_id '{}' not found in source notebook; skipping",
                    section.cell_id
                );
                continue;
            }
        };

        let original_source = src_cell.source_str();

        // Track whether the cell already had nota-bene metadata, so we can
        // preserve the key even when all fields end up absent.
        let had_nb = src_cell.nota_bene().is_some();

        // Strip any editor subkey that may have been left.
        clear_editor_meta(src_cell);

        let mut view = src_cell.nota_bene_mut();

        // ---- fixtures -------------------------------------------------------
        if !section.fixtures.is_empty() {
            let map: IndexMap<String, Fixture> = section.fixtures.iter().cloned().collect();
            view.set_fixtures(Some(map));
        } else {
            view.set_fixtures(None); // removes the key
        }

        // ---- diff -----------------------------------------------------------
        let diff = compute_diff(&original_source, &section.patched_source);
        view.set_diff(diff);

        // ---- test -----------------------------------------------------------
        let has_test_content = section
            .test_source
            .as_deref()
            .map(|s| !s.trim().is_empty())
            .unwrap_or(false);

        if has_test_content {
            view.set_test(Some(TestMeta {
                name: section
                    .test_name
                    .clone()
                    .unwrap_or_else(|| "<unnamed>".to_string()),
                source: section.test_source.clone().unwrap_or_default(),
            }));
        } else {
            view.set_test(None); // removes the key
        }

        // Preserve the nota-bene key if the cell already had it, even if all
        // sub-keys ended up absent (going through the editor IS the review).
        if had_nb {
            view.mark_addressed();
        }

        // Stamp the SHA snapshot for this cell — it has been reviewed by going
        // through the editor. Only stamp cells that have (or now have) nota-bene
        // metadata; plain passthrough cells stay clean.
        if src_cell.nota_bene().is_some() {
            let shas_slice = snapshot[..=cell_idx].to_vec();
            src_cell.nota_bene_mut().set_shas(shas_slice);
        }
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Section parser
// ---------------------------------------------------------------------------

fn parse_sections(editor: &Notebook) -> Result<Vec<Section>> {
    let mut sections: Vec<Section> = Vec::new();
    let mut current: Option<SectionBuilder> = None;

    for cell in &editor.cells {
        let role = cell.editor_role();
        let role = role.as_deref();

        match role {
            Some("setup") => continue,
            Some("section-header") => {
                if let Some(b) = current.take() {
                    sections.push(b.finish());
                }
                let cell_id = cell
                    .editor_cell_id()
                    .unwrap_or_else(|| cell.cell_id_str().to_string());
                current = Some(SectionBuilder::new(cell_id));
            }
            Some("fixture") => {
                if let Some(ref mut b) = current {
                    b.add_fixture(cell);
                }
            }
            Some("patched-source") | Some("source") => {
                if let Some(ref mut b) = current {
                    b.set_patched_source(cell.source_str());
                }
            }
            Some("test") => {
                if let Some(ref mut b) = current {
                    if b.test_source.is_some() {
                        eprintln!(
                            "warning: section '{}' has multiple test cells; \
                             only the last one will be used",
                            b.cell_id
                        );
                    }
                    b.set_test(cell);
                }
            }
            _ => {
                // Untagged cell: check if it's a code cell that could be a new
                // fixture (between section-header and patched-source) or a new test.
                if let Some(ref mut b) = current {
                    if let Cell::Code { .. } = cell {
                        if b.patched_source.is_none() {
                            // Before patched source → treat as new fixture
                            b.add_untagged_fixture(cell);
                        } else if b.test_source.is_none() {
                            // After patched source → treat as new test
                            b.set_test(cell);
                        }
                    } else {
                        // Non-code cells (markdown, raw) inserted by the user within a
                        // section are ignored during apply.
                        eprintln!(
                            "warning: non-code cell '{}' inside section '{}' will be ignored",
                            cell.cell_id_str(),
                            b.cell_id
                        );
                    }
                }
            }
        }
    }

    if let Some(b) = current.take() {
        sections.push(b.finish());
    }

    Ok(sections)
}

// ---------------------------------------------------------------------------
// SectionBuilder
// ---------------------------------------------------------------------------

struct SectionBuilder {
    cell_id: String,
    fixtures: Vec<(String, Fixture)>,
    patched_source: Option<String>,
    test_name: Option<String>,
    test_source: Option<String>,
}

impl SectionBuilder {
    fn new(cell_id: String) -> Self {
        Self {
            cell_id,
            fixtures: vec![],
            patched_source: None,
            test_name: None,
            test_source: None,
        }
    }

    fn add_fixture(&mut self, cell: &Cell) {
        let (name, fixture) = parse_fixture_cell(cell, &self.cell_id, self.fixtures.len());
        // Skip fixtures whose body is blank/whitespace-only.
        if !fixture.source.trim().is_empty() {
            self.fixtures.push((name, fixture));
        }
    }

    fn add_untagged_fixture(&mut self, cell: &Cell) {
        let (name, fixture) = parse_fixture_cell(cell, &self.cell_id, self.fixtures.len());
        if !fixture.source.trim().is_empty() {
            self.fixtures.push((name, fixture));
        }
    }

    fn set_patched_source(&mut self, src: String) {
        self.patched_source = Some(src);
    }

    fn set_test(&mut self, cell: &Cell) {
        let src = cell.source_str();
        // Strip %%nb_skip first line if present.
        let src = if let Some(stripped) = src.strip_prefix("%%nb_skip\n") {
            stripped.to_string()
        } else if src == "%%nb_skip" {
            String::new()
        } else {
            src
        };

        // Parse # test: line
        let (name, body) = if let Some(rest) = src.strip_prefix("# test:") {
            let name = rest
                .trim_start()
                .lines()
                .next()
                .unwrap_or("")
                .trim()
                .to_string();
            let body_start = src.find('\n').map(|i| i + 1).unwrap_or(src.len());
            (name, src[body_start..].to_string())
        } else {
            ("<unnamed>".to_string(), src)
        };

        self.test_name = Some(name);
        self.test_source = Some(body);
    }

    fn finish(self) -> Section {
        Section {
            cell_id: self.cell_id,
            fixtures: self.fixtures,
            patched_source: self.patched_source.unwrap_or_default(),
            test_name: self.test_name,
            test_source: self.test_source,
        }
    }
}

// ---------------------------------------------------------------------------
// Fixture cell parser
// ---------------------------------------------------------------------------

fn parse_fixture_cell(cell: &Cell, parent_cell_id: &str, position: usize) -> (String, Fixture) {
    let src = cell.source_str();
    let mut name: Option<String> = None;
    let mut description = String::new();
    let mut priority: Option<i64> = None;
    let mut byte_pos = 0usize;
    let raw_lines: Vec<&str> = src.lines().collect();
    let mut i = 0;
    while i < raw_lines.len() {
        let line = raw_lines[i];
        if let Some(rest) = line.strip_prefix("# fixture:") {
            name = Some(rest.trim().to_string());
            byte_pos += line.len() + 1; // +1 for newline
            i += 1;
        } else if let Some(rest) = line.strip_prefix("# description:") {
            description = rest.trim().to_string();
            byte_pos += line.len() + 1;
            i += 1;
        } else if let Some(rest) = line.strip_prefix("# priority:") {
            priority = rest.trim().parse().ok();
            byte_pos += line.len() + 1;
            i += 1;
        } else {
            break;
        }
    }

    let body = if byte_pos < src.len() {
        src[byte_pos..].to_string()
    } else {
        String::new()
    };

    let final_name = name.unwrap_or_else(|| format!("fixture_{}_{}", parent_cell_id, position));

    let final_priority = priority.unwrap_or(position as i64);

    (
        final_name,
        Fixture {
            description,
            priority: final_priority,
            source: body,
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::edit::split_source;
    use crate::notebook::blank_cell_metadata;
    use nbformat::v4::{Cell, CellId, Metadata, Notebook};
    use serde_json::json;

    fn cid(s: &str) -> CellId {
        CellId::new(s).unwrap()
    }

    /// Plain code cell with no nota-bene metadata.
    fn code_cell(id: &str, source: &str) -> Cell {
        Cell::Code {
            id: cid(id),
            metadata: blank_cell_metadata(),
            execution_count: None,
            source: split_source(source),
            outputs: vec![],
        }
    }

    /// Code cell that already has nota-bene metadata.
    fn code_cell_with_nb(id: &str, source: &str, nb_val: serde_json::Value) -> Cell {
        let mut meta = blank_cell_metadata();
        meta.additional.insert("nota-bene".to_string(), nb_val);
        Cell::Code {
            id: cid(id),
            metadata: meta,
            execution_count: None,
            source: split_source(source),
            outputs: vec![],
        }
    }

    /// Build a cell that carries an editor role in its nota-bene metadata.
    fn editor_cell(id: &str, source: &str, role: &str, target: &str) -> Cell {
        let mut meta = blank_cell_metadata();
        meta.additional.insert(
            "nota-bene".to_string(),
            json!({ "editor": { "role": role, "cell_id": target } }),
        );
        Cell::Code {
            id: cid(id),
            metadata: meta,
            execution_count: None,
            source: split_source(source),
            outputs: vec![],
        }
    }

    /// Markdown section-header cell pointing at `target`.
    fn section_header(id: &str, target: &str) -> Cell {
        let mut meta = blank_cell_metadata();
        meta.additional.insert(
            "nota-bene".to_string(),
            json!({ "editor": { "role": "section-header", "cell_id": target } }),
        );
        Cell::Markdown {
            id: cid(id),
            metadata: meta,
            source: vec!["## header".to_string()],
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

    /// Minimal editor notebook for one section: header + patched-source [+ optional test].
    fn simple_editor(target: &str, patched: &str, test_src: Option<&str>) -> Notebook {
        let mut cells = vec![
            section_header("hdr", target),
            editor_cell("ps", patched, "patched-source", target),
        ];
        if let Some(t) = test_src {
            cells.push(editor_cell("tst", t, "test", target));
        }
        notebook(cells)
    }

    // -------------------------------------------------------------------------
    // Fixture header parsing (via apply_editor_to_source)
    // -------------------------------------------------------------------------

    #[test]
    fn fixture_all_headers_parsed_correctly() {
        let mut source = notebook(vec![code_cell("src", "x = 1")]);
        let fixture_src = "# fixture: my_fix\n# description: My desc\n# priority: 5\nx = 10";
        let editor = notebook(vec![
            section_header("hdr", "src"),
            editor_cell("fix", fixture_src, "fixture", "src"),
            editor_cell("ps", "x = 1", "patched-source", "src"),
        ]);
        apply_editor_to_source(&mut source, &editor).unwrap();

        let data = source.cells[0].nota_bene().unwrap();
        let fixtures = data.fixtures.as_ref().unwrap();
        let fix = fixtures.get("my_fix").expect("fixture not found");
        assert_eq!(fix.description, "My desc");
        assert_eq!(fix.priority, 5);
        assert_eq!(fix.source, "x = 10");
    }

    #[test]
    fn fixture_missing_headers_uses_defaults() {
        let mut source = notebook(vec![code_cell("src", "x = 1")]);
        let editor = notebook(vec![
            section_header("hdr", "src"),
            editor_cell("fix", "x = 42", "fixture", "src"),
            editor_cell("ps", "x = 1", "patched-source", "src"),
        ]);
        apply_editor_to_source(&mut source, &editor).unwrap();

        let data = source.cells[0].nota_bene().unwrap();
        let fixtures = data.fixtures.as_ref().unwrap();
        assert_eq!(fixtures.len(), 1);
        let (name, fix) = fixtures.iter().next().unwrap();
        // Auto-generated name contains the parent cell id.
        assert!(name.contains("src"), "name was: {name}");
        assert_eq!(fix.source, "x = 42");
        assert_eq!(fix.priority, 0); // position 0
    }

    #[test]
    fn fixture_partial_headers_uses_what_is_present() {
        let mut source = notebook(vec![code_cell("src", "x = 1")]);
        // Only name header, no description or priority.
        let fixture_src = "# fixture: named_fix\nx = 7";
        let editor = notebook(vec![
            section_header("hdr", "src"),
            editor_cell("fix", fixture_src, "fixture", "src"),
            editor_cell("ps", "x = 1", "patched-source", "src"),
        ]);
        apply_editor_to_source(&mut source, &editor).unwrap();

        let data = source.cells[0].nota_bene().unwrap();
        let fixtures = data.fixtures.as_ref().unwrap();
        let fix = fixtures.get("named_fix").unwrap();
        assert_eq!(fix.description, ""); // empty default
        assert_eq!(fix.priority, 0); // positional default
        assert_eq!(fix.source, "x = 7");
    }

    // -------------------------------------------------------------------------
    // Test cell parsing
    // -------------------------------------------------------------------------

    #[test]
    fn test_cell_with_nb_skip_and_name() {
        let mut source = notebook(vec![code_cell("src", "x = 1")]);
        let editor = simple_editor(
            "src",
            "x = 1",
            Some("%%nb_skip\n# test: check_x\nassert x == 1"),
        );
        apply_editor_to_source(&mut source, &editor).unwrap();

        let data = source.cells[0].nota_bene().unwrap();
        let test = data.test.as_ref().unwrap();
        assert_eq!(test.name, "check_x");
        assert_eq!(test.source, "assert x == 1");
    }

    #[test]
    fn test_cell_bare_nb_skip_yields_no_test() {
        // "%%nb_skip" alone → body is empty → has_test_content = false → test key absent.
        let mut meta = blank_cell_metadata();
        meta.additional.insert("nota-bene".to_string(), json!({}));
        let src_cell = Cell::Code {
            id: cid("src"),
            metadata: meta,
            execution_count: None,
            source: split_source("x = 1"),
            outputs: vec![],
        };
        let mut source = notebook(vec![src_cell]);
        let editor = simple_editor("src", "x = 1", Some("%%nb_skip"));
        apply_editor_to_source(&mut source, &editor).unwrap();

        let data = source.cells[0].nota_bene().unwrap();
        // Blank test stub → test key absent.
        assert!(data.test.is_none());
    }

    #[test]
    fn test_cell_no_test_header_uses_unnamed() {
        let mut source = notebook(vec![code_cell("src", "x = 1")]);
        let editor = simple_editor("src", "x = 1", Some("%%nb_skip\nassert x == 1"));
        apply_editor_to_source(&mut source, &editor).unwrap();

        let data = source.cells[0].nota_bene().unwrap();
        let test = data.test.as_ref().unwrap();
        assert_eq!(test.name, "<unnamed>");
        assert_eq!(test.source, "assert x == 1");
    }

    #[test]
    fn test_cell_without_nb_skip_captures_whole_source_as_unnamed() {
        let mut source = notebook(vec![code_cell("src", "x = 1")]);
        let editor = simple_editor("src", "x = 1", Some("assert x == 1"));
        apply_editor_to_source(&mut source, &editor).unwrap();

        let data = source.cells[0].nota_bene().unwrap();
        let test = data.test.as_ref().unwrap();
        assert_eq!(test.name, "<unnamed>");
        assert_eq!(test.source, "assert x == 1");
    }

    // -------------------------------------------------------------------------
    // Diff semantics
    // -------------------------------------------------------------------------

    #[test]
    fn diff_set_when_patched_source_differs() {
        let mut source = notebook(vec![code_cell("src", "x = 1\n")]);
        let editor = simple_editor("src", "x = 99\n", None);
        apply_editor_to_source(&mut source, &editor).unwrap();

        let data = source.cells[0].nota_bene().unwrap();
        assert!(data.diff.is_some());
    }

    #[test]
    fn diff_absent_stays_absent_when_no_change_and_no_prior_meta() {
        // Source cell has no nota-bene; editor makes no change → no diff key created.
        let mut source = notebook(vec![code_cell("src", "x = 1")]);
        let editor = simple_editor("src", "x = 1", None);
        apply_editor_to_source(&mut source, &editor).unwrap();

        // Cell had no nota-bene and nothing was written → stays absent.
        assert!(source.cells[0].nota_bene().is_none());
    }

    #[test]
    fn diff_previously_set_becomes_absent_when_source_now_unchanged() {
        let mut source = notebook(vec![code_cell_with_nb(
            "src",
            "x = 1",
            json!({ "diff": "--- orig\n+++ patched\n" }),
        )]);
        let editor = simple_editor("src", "x = 1", None);
        apply_editor_to_source(&mut source, &editor).unwrap();

        let data = source.cells[0].nota_bene().unwrap();
        // Previously had a diff; now same source → diff key absent.
        assert!(data.diff.is_none());
    }

    // -------------------------------------------------------------------------
    // Fixture semantics
    // -------------------------------------------------------------------------

    #[test]
    fn fixtures_absent_stays_absent_when_editor_adds_none() {
        let mut source = notebook(vec![code_cell("src", "x = 1")]);
        let editor = simple_editor("src", "x = 1", None);
        apply_editor_to_source(&mut source, &editor).unwrap();

        // No nota-bene at all (prev was absent and nothing was written).
        assert!(source.cells[0].nota_bene().is_none());
    }

    #[test]
    fn fixtures_previously_set_now_removed_clears_key() {
        let mut source = notebook(vec![code_cell_with_nb(
            "src",
            "x = 1",
            json!({ "fixtures": { "f1": { "description": "d", "priority": 0, "source": "x" } } }),
        )]);
        let editor = simple_editor("src", "x = 1", None);
        apply_editor_to_source(&mut source, &editor).unwrap();

        let data = source.cells[0].nota_bene().unwrap();
        // Had fixtures → now no fixture cells → fixtures key absent.
        assert!(data.fixtures.is_none());
    }

    // -------------------------------------------------------------------------
    // Test semantics
    // -------------------------------------------------------------------------

    #[test]
    fn test_previously_set_now_empty_removes_key() {
        let mut source = notebook(vec![code_cell_with_nb(
            "src",
            "x = 1",
            json!({ "test": { "name": "old_test", "source": "assert True" } }),
        )]);
        // Editor has a test cell that is just %%nb_skip (empty body).
        let editor = simple_editor("src", "x = 1", Some("%%nb_skip"));
        apply_editor_to_source(&mut source, &editor).unwrap();

        let data = source.cells[0].nota_bene().unwrap();
        // Test cell present but blank → test key absent.
        assert!(data.test.is_none());
    }

    // -------------------------------------------------------------------------
    // Nota-bene key preservation
    // -------------------------------------------------------------------------

    #[test]
    fn nb_key_preserved_even_when_all_subkeys_absent() {
        let mut source = notebook(vec![code_cell_with_nb("src", "x = 1", json!({}))]);
        let editor = simple_editor("src", "x = 1", None);
        apply_editor_to_source(&mut source, &editor).unwrap();

        // Prior nota-bene was Some → nb key must still exist.
        assert!(source.cells[0].nota_bene().is_some());
    }

    // -------------------------------------------------------------------------
    // Unknown cell_id
    // -------------------------------------------------------------------------

    #[test]
    fn unknown_cell_id_skips_without_error() {
        let mut source = notebook(vec![code_cell("real-cell", "x = 1")]);
        let editor = simple_editor("ghost-cell", "x = 1", None);
        // Should not return an error; source cell stays untouched.
        apply_editor_to_source(&mut source, &editor).unwrap();
        assert!(source.cells[0].nota_bene().is_none());
    }

    // -------------------------------------------------------------------------
    // Multiple sections
    // -------------------------------------------------------------------------

    #[test]
    fn multiple_sections_applied_independently() {
        let mut source = notebook(vec![code_cell("c1", "x = 1\n"), code_cell("c2", "y = 2\n")]);
        let editor = notebook(vec![
            section_header("h1", "c1"),
            editor_cell("ps1", "x = 99\n", "patched-source", "c1"),
            section_header("h2", "c2"),
            editor_cell("ps2", "y = 2\n", "patched-source", "c2"),
        ]);
        apply_editor_to_source(&mut source, &editor).unwrap();

        // c1 should have a diff; c2 should not (and stays absent).
        let data1 = source.cells[0].nota_bene().unwrap();
        assert!(data1.diff.is_some());
        assert!(source.cells[1].nota_bene().is_none());
    }

    // -------------------------------------------------------------------------
    // Untagged cells used as fixtures / tests
    // -------------------------------------------------------------------------

    #[test]
    fn untagged_code_cell_before_patched_source_treated_as_fixture() {
        let mut source = notebook(vec![code_cell("src", "x = 1")]);
        // An untagged code cell inserted between section-header and patched-source
        // should be treated as a new fixture.
        let editor = notebook(vec![
            section_header("hdr", "src"),
            // No role tag — appears before patched-source.
            code_cell("untagged", "x = 42"),
            editor_cell("ps", "x = 1", "patched-source", "src"),
        ]);
        apply_editor_to_source(&mut source, &editor).unwrap();

        let data = source.cells[0].nota_bene().unwrap();
        let fixtures = data.fixtures.as_ref().unwrap();
        assert_eq!(fixtures.len(), 1);
    }

    // -------------------------------------------------------------------------
    // Multiple test cells
    // -------------------------------------------------------------------------

    #[test]
    fn multiple_tagged_test_cells_last_one_wins() {
        // When a section has two tagged test cells, the last one should be used.
        let mut source = notebook(vec![code_cell("src", "x = 1")]);
        let editor = notebook(vec![
            section_header("hdr", "src"),
            editor_cell("ps", "x = 1", "patched-source", "src"),
            editor_cell(
                "t1",
                "%%nb_skip\n# test: first_test\nassert x == 1",
                "test",
                "src",
            ),
            editor_cell(
                "t2",
                "%%nb_skip\n# test: second_test\nassert x == 2",
                "test",
                "src",
            ),
        ]);
        apply_editor_to_source(&mut source, &editor).unwrap();

        let data = source.cells[0].nota_bene().unwrap();
        let test = data.test.as_ref().unwrap();
        assert_eq!(
            test.name, "second_test",
            "expected last test cell to win, got: {}",
            test.name
        );
    }

    // -------------------------------------------------------------------------
    // Non-code cells (markdown / raw) inserted within a section
    // -------------------------------------------------------------------------

    /// A markdown cell inserted between section-header and patched-source should
    /// be silently ignored — not treated as a fixture.
    #[test]
    fn non_code_cell_before_patched_source_is_ignored() {
        let mut source = notebook(vec![code_cell("src", "x = 1")]);
        let editor = notebook(vec![
            section_header("hdr", "src"),
            // Markdown cell — user added a note; should be ignored.
            Cell::Markdown {
                id: cid("md-note"),
                metadata: blank_cell_metadata(),
                source: split_source("some notes"),
                attachments: None,
            },
            editor_cell("ps", "x = 1", "patched-source", "src"),
        ]);
        apply_editor_to_source(&mut source, &editor).unwrap();

        // The markdown cell must not have been treated as a fixture.
        // The source cell had no prior nota-bene and nothing was written, so it stays absent.
        assert!(
            source.cells[0].nota_bene().is_none(),
            "markdown cell was incorrectly treated as a fixture (cell should remain absent)"
        );
    }

    /// A markdown cell inserted after the patched-source cell should be ignored —
    /// not treated as a test.
    #[test]
    fn non_code_cell_after_patched_source_is_ignored() {
        let mut source = notebook(vec![code_cell("src", "x = 1")]);
        let editor = notebook(vec![
            section_header("hdr", "src"),
            editor_cell("ps", "x = 1", "patched-source", "src"),
            // Markdown cell after patched-source — should be ignored.
            Cell::Markdown {
                id: cid("md-after"),
                metadata: blank_cell_metadata(),
                source: split_source("# Analysis notes"),
                attachments: None,
            },
        ]);
        apply_editor_to_source(&mut source, &editor).unwrap();

        // No test should have been parsed from the markdown cell.
        assert!(
            source.cells[0].nota_bene().is_none(),
            "markdown cell was incorrectly treated as a test (cell should remain absent)"
        );
    }

    // -------------------------------------------------------------------------
    // Fixture edge cases
    // -------------------------------------------------------------------------

    /// A fixture cell with comment headers but no body is treated as a blank stub
    /// and filtered out. Fixtures key is absent.
    #[test]
    fn fixture_with_headers_only_and_empty_body_is_filtered() {
        let mut source = notebook(vec![code_cell("src", "x = 1")]);
        let fixture_src = "# fixture: empty_fix\n# description: no body here\n# priority: 0";
        let editor = notebook(vec![
            section_header("hdr", "src"),
            editor_cell("fix", fixture_src, "fixture", "src"),
            editor_cell("ps", "x = 1", "patched-source", "src"),
        ]);
        apply_editor_to_source(&mut source, &editor).unwrap();

        // Source cell had no prior nota-bene and the only fixture was blank →
        // nothing written, cell stays without nota-bene.
        assert!(
            source.cells[0].nota_bene().is_none(),
            "expected cell to remain absent when only blank-body fixtures are present"
        );
    }

    /// A fixture cell with whitespace-only body is also filtered.
    #[test]
    fn fixture_with_whitespace_only_body_is_filtered() {
        let mut source = notebook(vec![code_cell("src", "x = 1")]);
        let fixture_src = "# fixture: ws_fix\n   \n\t\n";
        let editor = notebook(vec![
            section_header("hdr", "src"),
            editor_cell("fix", fixture_src, "fixture", "src"),
            editor_cell("ps", "x = 1", "patched-source", "src"),
        ]);
        apply_editor_to_source(&mut source, &editor).unwrap();

        assert!(source.cells[0].nota_bene().is_none());
    }

    /// A `# fixture:` line with only whitespace after the colon but a real body
    /// should still produce a fixture (with an empty-string name).
    #[test]
    fn fixture_with_blank_name_but_real_body_is_kept() {
        let mut source = notebook(vec![code_cell("src", "x = 1")]);
        let fixture_src = "# fixture: \nx = 7";
        let editor = notebook(vec![
            section_header("hdr", "src"),
            editor_cell("fix", fixture_src, "fixture", "src"),
            editor_cell("ps", "x = 1", "patched-source", "src"),
        ]);
        apply_editor_to_source(&mut source, &editor).unwrap();

        let data = source.cells[0].nota_bene().unwrap();
        let fixtures = data.fixtures.as_ref().unwrap();
        assert_eq!(fixtures.len(), 1);
        let (name, fix) = fixtures.iter().next().unwrap();
        assert_eq!(fix.source, "x = 7");
        assert_eq!(name, "", "expected empty name from blank fixture header");
    }

    // -------------------------------------------------------------------------
    // Stub round-trip
    // -------------------------------------------------------------------------

    /// When the editor notebook contains a stub fixture cell (headers-only, empty
    /// body), leaving it blank produces no fixtures key.
    #[test]
    fn blank_stub_fixture_removes_fixtures_key() {
        // Source cell is Present with no fixtures (fixtures key absent).
        let mut source = notebook(vec![code_cell_with_nb("src", "x = 1", json!({}))]);
        // Editor contains the stub that `edit` would have emitted.
        let stub_src = "# fixture: \n# description: \n# priority: 0";
        let editor = notebook(vec![
            section_header("hdr", "src"),
            editor_cell("fix", stub_src, "fixture", "src"),
            editor_cell("ps", "x = 1", "patched-source", "src"),
            editor_cell("tst", "%%nb_skip\n# test: <unnamed>", "test", "src"),
        ]);
        apply_editor_to_source(&mut source, &editor).unwrap();

        let data = source.cells[0].nota_bene().unwrap();
        assert!(
            data.fixtures.is_none(),
            "blank stub fixture should produce absent fixtures key"
        );
    }

    /// When the editor notebook contains a stub test cell, leaving it blank produces
    /// no test key.
    #[test]
    fn blank_stub_test_removes_test_key() {
        let mut source = notebook(vec![code_cell_with_nb("src", "x = 1", json!({}))]);
        let editor = notebook(vec![
            section_header("hdr", "src"),
            editor_cell("ps", "x = 1", "patched-source", "src"),
            editor_cell("tst", "%%nb_skip\n# test: <unnamed>", "test", "src"),
        ]);
        apply_editor_to_source(&mut source, &editor).unwrap();

        let data = source.cells[0].nota_bene().unwrap();
        assert!(
            data.test.is_none(),
            "blank stub test should produce absent test key"
        );
    }

    /// Whitespace-only test body also counts as blank.
    #[test]
    fn whitespace_only_test_body_removes_test_key() {
        let mut source = notebook(vec![code_cell_with_nb("src", "x = 1", json!({}))]);
        let editor = notebook(vec![
            section_header("hdr", "src"),
            editor_cell("ps", "x = 1", "patched-source", "src"),
            editor_cell(
                "tst",
                "%%nb_skip\n# test: my_test\n   \n\t\n",
                "test",
                "src",
            ),
        ]);
        apply_editor_to_source(&mut source, &editor).unwrap();

        let data = source.cells[0].nota_bene().unwrap();
        assert!(
            data.test.is_none(),
            "whitespace-only test body should produce absent test key"
        );
    }

    /// A mix of one blank and one real fixture: the blank one is filtered, the
    /// real one is kept.
    #[test]
    fn mix_of_blank_and_real_fixtures_keeps_only_real() {
        let mut source = notebook(vec![code_cell("src", "x = 1")]);
        let editor = notebook(vec![
            section_header("hdr", "src"),
            // Blank stub.
            editor_cell(
                "fix1",
                "# fixture: \n# description: \n# priority: 0",
                "fixture",
                "src",
            ),
            // Real fixture.
            editor_cell("fix2", "# fixture: real_fix\nx = 7", "fixture", "src"),
            editor_cell("ps", "x = 1", "patched-source", "src"),
        ]);
        apply_editor_to_source(&mut source, &editor).unwrap();

        let data = source.cells[0].nota_bene().unwrap();
        let fixtures = data.fixtures.as_ref().unwrap();
        assert_eq!(fixtures.len(), 1, "only the real fixture should be kept");
        assert!(
            fixtures.contains_key("real_fix"),
            "real fixture should be present"
        );
    }
}
