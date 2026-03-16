use anyhow::{Context, Result};
use nbformat::v4::{Cell, ErrorOutput, Notebook, Output};
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::diff_utils::apply_diff;
use crate::edit::split_source;
use crate::metadata::Fixture;
use crate::notebook::{blank_cell_metadata, new_cell_id, CellExt};

// ---------------------------------------------------------------------------
// Public output types
// ---------------------------------------------------------------------------

#[derive(Debug, Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum CellTestResult {
    Completed {
        cell_id: String,
        test_name: String,
        subtests: Vec<SubtestResult>,
    },
    Error {
        cell_id: String,
        test_name: String,
        error: TestError,
    },
}

impl CellTestResult {
    pub fn all_passed(&self) -> bool {
        match self {
            CellTestResult::Completed { subtests, .. } => subtests.iter().all(|s| s.passed),
            CellTestResult::Error { .. } => false,
        }
    }

    pub fn is_error(&self) -> bool {
        matches!(self, CellTestResult::Error { .. })
    }
}

#[derive(Debug, Serialize, Deserialize)]
pub struct SubtestResult {
    pub name: String,
    pub passed: bool,
    pub error: Option<String>,
    pub traceback: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct TestError {
    pub phase: String,
    pub source_cell_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fixture_name: Option<String>,
    pub detail: String,
    pub traceback: Option<String>,
}

// ---------------------------------------------------------------------------
// Build test notebook
// ---------------------------------------------------------------------------

/// Generate a self-contained test notebook for the cell at `target_idx`.
///
/// The notebook walks every code cell from 0..=target_idx in order:
/// fixtures (wrapped in a function, sorted by priority) then load_cell + execute_cell.
/// After the target cell's execute_cell, the test source runs (if any),
/// then results are collected and teardown fires.
pub fn build_test_notebook(source: &Notebook, target_idx: usize) -> Result<Notebook> {
    let mut cells: Vec<Cell> = Vec::new();

    // Setup cell
    cells.push(make_setup_cell());

    for (idx, cell) in source.cells.iter().enumerate() {
        // skip non-code cells entirely
        let cell = match cell {
            Cell::Code { .. } => cell,
            _ => continue,
        };

        // target_idx is the index in the full cells list (matching main.rs enumerate)
        // so we skip code cells that come after the target
        if idx > target_idx {
            break;
        }

        let cell_id = cell.cell_id_str().to_string();
        let nb_meta = cell.nota_bene();

        // Fixtures (only if metadata present and fixtures are defined)
        if let Some(ref data) = nb_meta {
            if let Some(fixtures) = &data.fixtures {
                let mut sorted: Vec<(&String, &Fixture)> = fixtures.iter().collect();
                sorted.sort_by_key(|(_, f)| f.priority);
                for (name, fixture) in sorted {
                    cells.push(make_fixture_cell(&cell_id, name, fixture));
                }
            }
        }

        // Patched source
        let patched_source = match nb_meta.as_ref().and_then(|d| d.diff.as_deref()) {
            Some(diff) => {
                apply_diff(&cell.source_str(), diff).unwrap_or_else(|_| cell.source_str())
            }
            None => cell.source_str(),
        };

        cells.push(make_cell_source_cell(&cell_id, &patched_source));

        // Test cell (target only)
        if idx == target_idx {
            if let Some(ref data) = nb_meta {
                if let Some(test) = &data.test {
                    cells.push(make_test_cell(&cell_id, &test.source));
                }
            }
        }
    }

    // Results + teardown
    cells.push(make_results_cell());
    cells.push(make_teardown_cell());

    let mut metadata = source.metadata.clone();
    // Strip any nota-bene notebook-level metadata — not needed for execution
    metadata.additional.remove("nota-bene");

    Ok(Notebook {
        metadata,
        nbformat: 4,
        nbformat_minor: 5,
        cells,
    })
}

// ---------------------------------------------------------------------------
// Cell constructors
// ---------------------------------------------------------------------------

fn make_setup_cell() -> Cell {
    let mut meta = blank_cell_metadata();
    meta.additional.insert(
        "nota-bene".to_string(),
        json!({"runner": {"role": "setup"}}),
    );
    Cell::Code {
        id: new_cell_id(),
        metadata: meta,
        execution_count: None,
        source: split_source("import nota_bene"),
        outputs: vec![],
    }
}

fn make_fixture_cell(source_cell_id: &str, fixture_name: &str, fixture: &Fixture) -> Cell {
    let indented = fixture
        .source
        .lines()
        .map(|l| format!("    {l}"))
        .collect::<Vec<_>>()
        .join("\n");
    let source =
        format!("def _nb_fixture_{fixture_name}():\n{indented}\n\n_nb_fixture_{fixture_name}()");
    let mut meta = blank_cell_metadata();
    meta.additional.insert(
        "nota-bene".to_string(),
        json!({
            "runner": {
                "role": "fixture",
                "source_cell_id": source_cell_id,
                "fixture_name": fixture_name,
            }
        }),
    );
    Cell::Code {
        id: new_cell_id(),
        metadata: meta,
        execution_count: None,
        source: split_source(&source),
        outputs: vec![],
    }
}

fn make_cell_source_cell(source_cell_id: &str, patched_source: &str) -> Cell {
    // Encode the patched source as a JSON string literal so it's safe to embed.
    let encoded = serde_json::to_string(patched_source).unwrap_or_else(|_| "\"\"".to_string());
    let source = format!("nota_bene._runner.load_cell({encoded})\nnota_bene.execute_cell()");
    let mut meta = blank_cell_metadata();
    meta.additional.insert(
        "nota-bene".to_string(),
        json!({
            "runner": {
                "role": "cell_source",
                "source_cell_id": source_cell_id,
            }
        }),
    );
    Cell::Code {
        id: new_cell_id(),
        metadata: meta,
        execution_count: None,
        source: split_source(&source),
        outputs: vec![],
    }
}

fn make_test_cell(source_cell_id: &str, test_source: &str) -> Cell {
    let mut meta = blank_cell_metadata();
    meta.additional.insert(
        "nota-bene".to_string(),
        json!({
            "runner": {
                "role": "test",
                "source_cell_id": source_cell_id,
            }
        }),
    );
    Cell::Code {
        id: new_cell_id(),
        metadata: meta,
        execution_count: None,
        source: split_source(test_source),
        outputs: vec![],
    }
}

fn make_results_cell() -> Cell {
    let mut meta = blank_cell_metadata();
    meta.additional.insert(
        "nota-bene".to_string(),
        json!({"runner": {"role": "results"}}),
    );
    Cell::Code {
        id: new_cell_id(),
        metadata: meta,
        execution_count: None,
        source: split_source(
            "import json as _json\nprint(\"__NB_RESULTS__\" + nota_bene._runner.get_test_results())",
        ),
        outputs: vec![],
    }
}

fn make_teardown_cell() -> Cell {
    let mut meta = blank_cell_metadata();
    meta.additional.insert(
        "nota-bene".to_string(),
        json!({"runner": {"role": "teardown"}}),
    );
    Cell::Code {
        id: new_cell_id(),
        metadata: meta,
        execution_count: None,
        source: split_source("nota_bene._runner.run_teardowns()"),
        outputs: vec![],
    }
}

// ---------------------------------------------------------------------------
// Result extraction
// ---------------------------------------------------------------------------

const RESULTS_MARKER: &str = "__NB_RESULTS__";

/// Extract role from a cell's nota-bene runner metadata.
fn runner_role(cell: &Cell) -> Option<String> {
    let nb = cell.additional().get("nota-bene")?;
    nb.get("runner")?
        .get("role")?
        .as_str()
        .map(|s| s.to_string())
}

fn runner_source_cell_id(cell: &Cell) -> Option<String> {
    let nb = cell.additional().get("nota-bene")?;
    nb.get("runner")?
        .get("source_cell_id")?
        .as_str()
        .map(|s| s.to_string())
}

fn runner_fixture_name(cell: &Cell) -> Option<String> {
    let nb = cell.additional().get("nota-bene")?;
    nb.get("runner")?
        .get("fixture_name")?
        .as_str()
        .map(|s| s.to_string())
}

/// Format an ErrorOutput into a human-readable detail string and traceback.
fn format_error(err: &ErrorOutput) -> (String, String) {
    let detail = format!("{}: {}", err.ename, err.evalue);
    let traceback = err.traceback.join("\n");
    (detail, traceback)
}

/// Find the first error in a `fixture` or `cell_source` cell — these are
/// infrastructure failures that invalidate the test regardless of what the
/// results cell reports.
fn find_setup_error(executed: &Notebook) -> Option<TestError> {
    for cell in &executed.cells {
        let Cell::Code { outputs, .. } = cell else {
            continue;
        };
        let role = runner_role(cell);
        if !matches!(role.as_deref(), Some("fixture") | Some("cell_source")) {
            continue;
        }
        for output in outputs {
            if let Output::Error(err) = output {
                let source_cell_id = runner_source_cell_id(cell);
                let fixture_name = runner_fixture_name(cell);
                let (detail, traceback) = format_error(err);
                return Some(TestError {
                    phase: role.unwrap(),
                    source_cell_id,
                    fixture_name,
                    detail,
                    traceback: Some(traceback),
                });
            }
        }
    }
    None
}

/// Find the first cell error output in the executed notebook, returning phase info.
fn find_first_cell_error(executed: &Notebook) -> Option<TestError> {
    for cell in &executed.cells {
        let Cell::Code { outputs, .. } = cell else {
            continue;
        };
        for output in outputs {
            if let Output::Error(err) = output {
                let role = runner_role(cell).unwrap_or_else(|| "unknown".to_string());
                let source_cell_id = runner_source_cell_id(cell);
                let fixture_name = runner_fixture_name(cell);
                let (detail, traceback) = format_error(err);
                return Some(TestError {
                    phase: role,
                    source_cell_id,
                    fixture_name,
                    detail,
                    traceback: Some(traceback),
                });
            }
        }
    }
    None
}

/// Scan the results cell's stdout for the `__NB_RESULTS__` marker and return
/// the JSON substring that follows it, or `None` if absent.
fn find_results_json(executed: &Notebook) -> Option<String> {
    let results_cell = executed
        .cells
        .iter()
        .find(|c| runner_role(c).as_deref() == Some("results"))?;
    let Cell::Code { outputs, .. } = results_cell else {
        return None;
    };
    for output in outputs {
        if let Output::Stream { name, text } = output {
            if name == "stdout" {
                if let Some(json_part) = text.0.as_str().strip_prefix(RESULTS_MARKER) {
                    return Some(json_part.trim().to_string());
                }
            }
        }
    }
    None
}

/// Build a single-entry subtest list for the implicit (no-subtest) case:
/// pass if the test cell had no error, fail otherwise.
fn implicit_subtest(executed: &Notebook, test_name: &str) -> Vec<SubtestResult> {
    let test_cell_error = executed.cells.iter().find_map(|c| {
        if runner_role(c).as_deref() != Some("test") {
            return None;
        }
        let Cell::Code { outputs, .. } = c else {
            return None;
        };
        outputs.iter().find_map(|o| {
            if let Output::Error(e) = o {
                Some(e.clone())
            } else {
                None
            }
        })
    });

    match test_cell_error {
        None => vec![SubtestResult {
            name: test_name.to_string(),
            passed: true,
            error: None,
            traceback: None,
        }],
        Some(err) => {
            let (detail, traceback) = format_error(&err);
            vec![SubtestResult {
                name: test_name.to_string(),
                passed: false,
                error: Some(detail),
                traceback: Some(traceback),
            }]
        }
    }
}

/// Extract subtest results from the executed notebook.
///
/// Returns Ok(CellTestResult) in all cases — infrastructure errors become
/// CellTestResult::Error, not Err(anyhow).
pub fn extract_results(executed: &Notebook, cell_id: &str, test_name: &str) -> CellTestResult {
    let make_error = |error: TestError| CellTestResult::Error {
        cell_id: cell_id.to_string(),
        test_name: test_name.to_string(),
        error,
    };

    // Fixture / cell_source errors take priority over everything else.
    if let Some(error) = find_setup_error(executed) {
        return make_error(error);
    }

    // Find the __NB_RESULTS__ JSON, or fall back to any cell error / sentinel.
    let Some(json_str) = find_results_json(executed) else {
        let has_results_cell = executed
            .cells
            .iter()
            .any(|c| runner_role(c).as_deref() == Some("results"));
        let fallback = find_first_cell_error(executed).unwrap_or_else(|| {
            if has_results_cell {
                TestError {
                    phase: "results".to_string(),
                    source_cell_id: None,
                    fixture_name: None,
                    detail: "Results cell produced no output".to_string(),
                    traceback: None,
                }
            } else {
                TestError {
                    phase: "executor".to_string(),
                    source_cell_id: None,
                    fixture_name: None,
                    detail: "Results cell missing from executed notebook".to_string(),
                    traceback: None,
                }
            }
        });
        return make_error(fallback);
    };

    // Parse the JSON.
    let mut subtests = match serde_json::from_str::<Vec<SubtestResult>>(&json_str) {
        Ok(s) => s,
        Err(e) => {
            return make_error(TestError {
                phase: "results".to_string(),
                source_cell_id: None,
                fixture_name: None,
                detail: format!("Failed to parse results JSON: {e}"),
                traceback: None,
            });
        }
    };

    // Empty list means the test used no subtest API — synthesise one entry.
    if subtests.is_empty() {
        subtests = implicit_subtest(executed, test_name);
    }

    CellTestResult::Completed {
        cell_id: cell_id.to_string(),
        test_name: test_name.to_string(),
        subtests,
    }
}

/// Parse the stdout of the executor subprocess into an executed notebook.
pub fn parse_executed_notebook(stdout: &str) -> Result<Notebook> {
    match nbformat::parse_notebook(stdout)
        .context("parsing executed notebook from subprocess stdout")?
    {
        nbformat::Notebook::V4(nb) => Ok(nb),
        nbformat::Notebook::Legacy(legacy) => nbformat::upgrade_legacy_notebook(legacy)
            .context("upgrading legacy notebook from executor"),
        nbformat::Notebook::V3(v3) => {
            nbformat::upgrade_v3_notebook(v3).context("upgrading v3 notebook from executor")
        }
    }
}

/// Build an error result from executor subprocess failure (bad exit, no stdout).
pub fn executor_error_result(cell_id: &str, test_name: &str, detail: &str) -> CellTestResult {
    CellTestResult::Error {
        cell_id: cell_id.to_string(),
        test_name: test_name.to_string(),
        error: TestError {
            phase: "executor".to_string(),
            source_cell_id: None,
            fixture_name: None,
            detail: detail.to_string(),
            traceback: None,
        },
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::notebook::blank_cell_metadata;
    use nbformat::v4::{Cell, CellId, Metadata, MultilineString, Notebook};

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

    fn notebook_from(cells: Vec<Cell>) -> Notebook {
        Notebook {
            metadata: Metadata::default(),
            nbformat: 4,
            nbformat_minor: 5,
            cells,
        }
    }

    fn cell_with_meta(id: &str, source: &str, meta_json: serde_json::Value) -> Cell {
        let mut meta = blank_cell_metadata();
        meta.additional.insert("nota-bene".to_string(), meta_json);
        Cell::Code {
            id: cid(id),
            metadata: meta,
            execution_count: None,
            source: split_source(source),
            outputs: vec![],
        }
    }

    // --- build_test_notebook ---

    #[test]
    fn first_cell_is_setup() {
        let nb = notebook_from(vec![code_cell("c1", "x = 1")]);
        let test_nb = build_test_notebook(&nb, 0).unwrap();
        assert_eq!(runner_role(&test_nb.cells[0]).as_deref(), Some("setup"));
    }

    #[test]
    fn setup_imports_nota_bene() {
        let nb = notebook_from(vec![code_cell("c1", "x = 1")]);
        let test_nb = build_test_notebook(&nb, 0).unwrap();
        assert_eq!(test_nb.cells[0].source_str(), "import nota_bene");
    }

    #[test]
    fn last_cells_are_results_and_teardown() {
        let nb = notebook_from(vec![code_cell("c1", "x = 1")]);
        let test_nb = build_test_notebook(&nb, 0).unwrap();
        let n = test_nb.cells.len();
        assert_eq!(
            runner_role(&test_nb.cells[n - 2]).as_deref(),
            Some("results")
        );
        assert_eq!(
            runner_role(&test_nb.cells[n - 1]).as_deref(),
            Some("teardown")
        );
    }

    #[test]
    fn cell_without_metadata_gets_cell_source_cell() {
        let nb = notebook_from(vec![code_cell("c1", "x = 1")]);
        let test_nb = build_test_notebook(&nb, 0).unwrap();
        let cell_source = test_nb
            .cells
            .iter()
            .find(|c| runner_role(c).as_deref() == Some("cell_source"))
            .expect("cell_source cell");
        let src = cell_source.source_str();
        assert!(src.contains("load_cell"));
        assert!(src.contains("execute_cell"));
        assert!(src.contains("x = 1"));
    }

    #[test]
    fn fixture_wrapped_in_function() {
        let meta = serde_json::json!({
            "fixtures": {
                "my_fix": {"description": "d", "priority": 0, "source": "x = 1\ny = 2"}
            }
        });
        let nb = notebook_from(vec![cell_with_meta("c1", "z = x + y", meta)]);
        let test_nb = build_test_notebook(&nb, 0).unwrap();
        let fixture_cell = test_nb
            .cells
            .iter()
            .find(|c| runner_role(c).as_deref() == Some("fixture"))
            .expect("fixture cell");
        let src = fixture_cell.source_str();
        assert!(src.contains("def _nb_fixture_my_fix():"));
        assert!(src.contains("    x = 1"));
        assert!(src.contains("_nb_fixture_my_fix()"));
    }

    #[test]
    fn fixtures_sorted_by_priority() {
        let meta = serde_json::json!({
            "fixtures": {
                "z_fix": {"description": "d", "priority": 10, "source": "z = 10"},
                "a_fix": {"description": "d", "priority": 1, "source": "a = 1"}
            }
        });
        let nb = notebook_from(vec![cell_with_meta("c1", "r = a + z", meta)]);
        let test_nb = build_test_notebook(&nb, 0).unwrap();
        let fixture_cells: Vec<_> = test_nb
            .cells
            .iter()
            .filter(|c| runner_role(c).as_deref() == Some("fixture"))
            .collect();
        assert_eq!(fixture_cells.len(), 2);
        assert!(fixture_cells[0].source_str().contains("a_fix"));
        assert!(fixture_cells[1].source_str().contains("z_fix"));
    }

    #[test]
    fn test_cell_emitted_for_target_only() {
        let meta = serde_json::json!({
            "test": {"name": "my test", "source": "assert x == 1"}
        });
        // Two cells: c1 (no test), c2 (has test). Target is c2 (idx 1).
        let nb = notebook_from(vec![
            code_cell("c1", "x = 1"),
            cell_with_meta("c2", "y = x + 1", meta),
        ]);
        let test_nb = build_test_notebook(&nb, 1).unwrap();
        let test_cells: Vec<_> = test_nb
            .cells
            .iter()
            .filter(|c| runner_role(c).as_deref() == Some("test"))
            .collect();
        assert_eq!(test_cells.len(), 1);
        assert!(test_cells[0].source_str().contains("assert x == 1"));
    }

    #[test]
    fn preceding_cells_included_in_chain() {
        let nb = notebook_from(vec![
            code_cell("c1", "x = 1"),
            code_cell("c2", "y = 2"),
            code_cell("c3", "z = 3"),
        ]);
        // target is c3 (idx 2): all three should appear as cell_source cells
        let test_nb = build_test_notebook(&nb, 2).unwrap();
        let cell_sources: Vec<_> = test_nb
            .cells
            .iter()
            .filter(|c| runner_role(c).as_deref() == Some("cell_source"))
            .collect();
        assert_eq!(cell_sources.len(), 3);
    }

    #[test]
    fn cells_after_target_not_included() {
        let nb = notebook_from(vec![
            code_cell("c1", "x = 1"),
            code_cell("c2", "y = 2"),
            code_cell("c3", "z = 3"),
        ]);
        // target is c1 (idx 0): only c1 should be a cell_source
        let test_nb = build_test_notebook(&nb, 0).unwrap();
        let cell_sources: Vec<_> = test_nb
            .cells
            .iter()
            .filter(|c| runner_role(c).as_deref() == Some("cell_source"))
            .collect();
        assert_eq!(cell_sources.len(), 1);
    }

    #[test]
    fn markdown_cells_skipped() {
        use nbformat::v4::CellId;
        let md = Cell::Markdown {
            id: CellId::new("md1").unwrap(),
            metadata: blank_cell_metadata(),
            source: split_source("# Title"),
            attachments: None,
        };
        // md is at index 0, c1 is at index 1 in the full cells list
        let nb = notebook_from(vec![md, code_cell("c1", "x = 1")]);
        let test_nb = build_test_notebook(&nb, 1).unwrap();
        // setup, cell_source(c1), results, teardown — no markdown cell
        assert_eq!(test_nb.cells.len(), 4);
    }

    #[test]
    fn diff_applied_in_cell_source() {
        let original = "x = 1\n";
        let patched = "x = 99\n";
        let diff = crate::diff_utils::compute_diff(original, patched).unwrap();
        let meta = serde_json::json!({ "diff": diff });
        let nb = notebook_from(vec![cell_with_meta("c1", original, meta)]);
        let test_nb = build_test_notebook(&nb, 0).unwrap();
        let cell_source = test_nb
            .cells
            .iter()
            .find(|c| runner_role(c).as_deref() == Some("cell_source"))
            .unwrap();
        assert!(cell_source.source_str().contains("x = 99"));
    }

    // --- extract_results ---

    fn make_executed_nb_with_results(results_json: &str) -> Notebook {
        let mut results_cell_meta = blank_cell_metadata();
        results_cell_meta.additional.insert(
            "nota-bene".to_string(),
            json!({"runner": {"role": "results"}}),
        );
        let results_cell = Cell::Code {
            id: cid("res"),
            metadata: results_cell_meta,
            execution_count: None,
            source: vec![],
            outputs: vec![Output::Stream {
                name: "stdout".to_string(),
                text: MultilineString(format!("{RESULTS_MARKER}{results_json}")),
            }],
        };
        notebook_from(vec![results_cell])
    }

    #[test]
    fn extract_results_happy_path() {
        let nb = make_executed_nb_with_results(
            r#"[{"name":"t1","passed":true,"error":null,"traceback":null}]"#,
        );
        let result = extract_results(&nb, "c1", "my test");
        match result {
            CellTestResult::Completed { subtests, .. } => {
                assert_eq!(subtests.len(), 1);
                assert!(subtests[0].passed);
            }
            CellTestResult::Error { .. } => panic!("expected Completed"),
        }
    }

    #[test]
    fn extract_results_implicit_pass() {
        // Empty results list + no error on test cell → implicit pass
        let nb = make_executed_nb_with_results("[]");
        let result = extract_results(&nb, "c1", "my test");
        match result {
            CellTestResult::Completed { subtests, .. } => {
                assert_eq!(subtests.len(), 1);
                assert!(subtests[0].passed);
                assert_eq!(subtests[0].name, "my test");
            }
            CellTestResult::Error { .. } => panic!("expected Completed"),
        }
    }

    #[test]
    fn extract_results_implicit_fail_from_test_cell_error() {
        // Empty results list + error on test cell → implicit fail
        let mut test_cell_meta = blank_cell_metadata();
        test_cell_meta.additional.insert(
            "nota-bene".to_string(),
            json!({"runner": {"role": "test", "source_cell_id": "c1"}}),
        );
        let test_cell = Cell::Code {
            id: cid("test"),
            metadata: test_cell_meta,
            execution_count: None,
            source: vec![],
            outputs: vec![Output::Error(ErrorOutput {
                ename: "AssertionError".to_string(),
                evalue: "x != 1".to_string(),
                traceback: vec!["line 1".to_string()],
            })],
        };
        let mut results_cell_meta = blank_cell_metadata();
        results_cell_meta.additional.insert(
            "nota-bene".to_string(),
            json!({"runner": {"role": "results"}}),
        );
        let results_cell = Cell::Code {
            id: cid("res"),
            metadata: results_cell_meta,
            execution_count: None,
            source: vec![],
            outputs: vec![Output::Stream {
                name: "stdout".to_string(),
                text: MultilineString(format!("{RESULTS_MARKER}[]")),
            }],
        };
        let nb = notebook_from(vec![test_cell, results_cell]);
        let result = extract_results(&nb, "c1", "my test");
        match result {
            CellTestResult::Completed { subtests, .. } => {
                assert_eq!(subtests.len(), 1);
                assert!(!subtests[0].passed);
                assert!(subtests[0]
                    .error
                    .as_deref()
                    .unwrap()
                    .contains("AssertionError"));
            }
            CellTestResult::Error { .. } => panic!("expected Completed"),
        }
    }

    #[test]
    fn extract_results_fixture_error() {
        // No results cell output — fixture errored before results could run.
        let mut fixture_meta = blank_cell_metadata();
        fixture_meta.additional.insert(
            "nota-bene".to_string(),
            json!({
                "runner": {
                    "role": "fixture",
                    "source_cell_id": "c1",
                    "fixture_name": "my_fix"
                }
            }),
        );
        let fixture_cell = Cell::Code {
            id: cid("fix"),
            metadata: fixture_meta,
            execution_count: None,
            source: vec![],
            outputs: vec![Output::Error(ErrorOutput {
                ename: "NameError".to_string(),
                evalue: "name 'foo' is not defined".to_string(),
                traceback: vec![],
            })],
        };
        // Results cell with no output (never ran)
        let mut results_meta = blank_cell_metadata();
        results_meta.additional.insert(
            "nota-bene".to_string(),
            json!({"runner": {"role": "results"}}),
        );
        let results_cell = Cell::Code {
            id: cid("res"),
            metadata: results_meta,
            execution_count: None,
            source: vec![],
            outputs: vec![],
        };
        let nb = notebook_from(vec![fixture_cell, results_cell]);
        let result = extract_results(&nb, "c1", "my test");
        match result {
            CellTestResult::Error { error, .. } => {
                assert_eq!(error.phase, "fixture");
                assert_eq!(error.fixture_name.as_deref(), Some("my_fix"));
                assert!(error.detail.contains("NameError"));
            }
            CellTestResult::Completed { .. } => panic!("expected Error"),
        }
    }

    #[test]
    fn cell_test_result_all_passed() {
        let r = CellTestResult::Completed {
            cell_id: "c1".to_string(),
            test_name: "t".to_string(),
            subtests: vec![
                SubtestResult {
                    name: "a".to_string(),
                    passed: true,
                    error: None,
                    traceback: None,
                },
                SubtestResult {
                    name: "b".to_string(),
                    passed: true,
                    error: None,
                    traceback: None,
                },
            ],
        };
        assert!(r.all_passed());
    }

    #[test]
    fn cell_test_result_not_all_passed_when_one_fails() {
        let r = CellTestResult::Completed {
            cell_id: "c1".to_string(),
            test_name: "t".to_string(),
            subtests: vec![
                SubtestResult {
                    name: "a".to_string(),
                    passed: true,
                    error: None,
                    traceback: None,
                },
                SubtestResult {
                    name: "b".to_string(),
                    passed: false,
                    error: Some("boom".to_string()),
                    traceback: None,
                },
            ],
        };
        assert!(!r.all_passed());
    }

    #[test]
    fn error_result_not_all_passed() {
        let r = CellTestResult::Error {
            cell_id: "c1".to_string(),
            test_name: "t".to_string(),
            error: TestError {
                phase: "executor".to_string(),
                source_cell_id: None,
                fixture_name: None,
                detail: "oops".to_string(),
                traceback: None,
            },
        };
        assert!(!r.all_passed());
        assert!(r.is_error());
    }

    // --- find_setup_error / setup error priority ---

    /// A fixture error must yield status=error even when the results cell
    /// also ran and produced output (allow_errors=True lets nbclient continue).
    #[test]
    fn fixture_error_takes_priority_over_results_output() {
        let mut fixture_meta = blank_cell_metadata();
        fixture_meta.additional.insert(
            "nota-bene".to_string(),
            json!({
                "runner": {
                    "role": "fixture",
                    "source_cell_id": "c1",
                    "fixture_name": "bad_fix",
                }
            }),
        );
        let fixture_cell = Cell::Code {
            id: cid("fix"),
            metadata: fixture_meta,
            execution_count: None,
            source: vec![],
            outputs: vec![Output::Error(ErrorOutput {
                ename: "RuntimeError".to_string(),
                evalue: "boom".to_string(),
                traceback: vec![],
            })],
        };
        // Results cell ran anyway (allow_errors=True) and output passing results
        let mut results_meta = blank_cell_metadata();
        results_meta.additional.insert(
            "nota-bene".to_string(),
            json!({"runner": {"role": "results"}}),
        );
        let results_cell = Cell::Code {
            id: cid("res"),
            metadata: results_meta,
            execution_count: None,
            source: vec![],
            outputs: vec![Output::Stream {
                name: "stdout".to_string(),
                text: MultilineString(format!(
                    "{RESULTS_MARKER}[{{\"name\":\"t\",\"passed\":true,\"error\":null,\"traceback\":null}}]"
                )),
            }],
        };
        let nb = notebook_from(vec![fixture_cell, results_cell]);
        let result = extract_results(&nb, "c1", "my test");
        match result {
            CellTestResult::Error { error, .. } => {
                assert_eq!(error.phase, "fixture");
                assert_eq!(error.fixture_name.as_deref(), Some("bad_fix"));
            }
            CellTestResult::Completed { .. } => {
                panic!("fixture error must not be reported as Completed")
            }
        }
    }

    /// A cell_source error must yield status=error even when results ran.
    #[test]
    fn cell_source_error_takes_priority_over_results_output() {
        let mut cell_source_meta = blank_cell_metadata();
        cell_source_meta.additional.insert(
            "nota-bene".to_string(),
            json!({"runner": {"role": "cell_source", "source_cell_id": "upstream"}}),
        );
        let cell_source_cell = Cell::Code {
            id: cid("src"),
            metadata: cell_source_meta,
            execution_count: None,
            source: vec![],
            outputs: vec![Output::Error(ErrorOutput {
                ename: "ValueError".to_string(),
                evalue: "bad input".to_string(),
                traceback: vec![],
            })],
        };
        let mut results_meta = blank_cell_metadata();
        results_meta.additional.insert(
            "nota-bene".to_string(),
            json!({"runner": {"role": "results"}}),
        );
        let results_cell = Cell::Code {
            id: cid("res"),
            metadata: results_meta,
            execution_count: None,
            source: vec![],
            outputs: vec![Output::Stream {
                name: "stdout".to_string(),
                text: MultilineString(format!("{RESULTS_MARKER}[]")),
            }],
        };
        let nb = notebook_from(vec![cell_source_cell, results_cell]);
        let result = extract_results(&nb, "c1", "my test");
        match result {
            CellTestResult::Error { error, .. } => {
                assert_eq!(error.phase, "cell_source");
                assert_eq!(error.source_cell_id.as_deref(), Some("upstream"));
            }
            CellTestResult::Completed { .. } => {
                panic!("cell_source error must not be reported as Completed")
            }
        }
    }

    /// An error in the test cell itself does NOT count as a setup error —
    /// it is handled via the implicit subtest path.
    #[test]
    fn test_cell_error_does_not_trigger_setup_error() {
        let mut test_meta = blank_cell_metadata();
        test_meta.additional.insert(
            "nota-bene".to_string(),
            json!({"runner": {"role": "test", "source_cell_id": "c1"}}),
        );
        let test_cell = Cell::Code {
            id: cid("test"),
            metadata: test_meta,
            execution_count: None,
            source: vec![],
            outputs: vec![Output::Error(ErrorOutput {
                ename: "AssertionError".to_string(),
                evalue: "x != 1".to_string(),
                traceback: vec!["line 1".to_string()],
            })],
        };
        let mut results_meta = blank_cell_metadata();
        results_meta.additional.insert(
            "nota-bene".to_string(),
            json!({"runner": {"role": "results"}}),
        );
        let results_cell = Cell::Code {
            id: cid("res"),
            metadata: results_meta,
            execution_count: None,
            source: vec![],
            outputs: vec![Output::Stream {
                name: "stdout".to_string(),
                text: MultilineString(format!("{RESULTS_MARKER}[]")),
            }],
        };
        let nb = notebook_from(vec![test_cell, results_cell]);
        let result = extract_results(&nb, "c1", "my test");
        // Should be Completed (implicit fail), not Error
        match result {
            CellTestResult::Completed { subtests, .. } => {
                assert_eq!(subtests.len(), 1);
                assert!(!subtests[0].passed);
            }
            CellTestResult::Error { .. } => {
                panic!("test cell error should produce implicit fail, not infrastructure error")
            }
        }
    }

    // --- no-results-output branches (branches 2-5) ---

    /// Results cell is present but produced no __NB_RESULTS__ output, and
    /// another cell (e.g. the test cell) has an error output → that error
    /// is surfaced.
    #[test]
    fn results_cell_present_no_marker_with_cell_error() {
        let mut test_meta = blank_cell_metadata();
        test_meta.additional.insert(
            "nota-bene".to_string(),
            json!({"runner": {"role": "test", "source_cell_id": "c1"}}),
        );
        let test_cell = Cell::Code {
            id: cid("test"),
            metadata: test_meta,
            execution_count: None,
            source: vec![],
            outputs: vec![Output::Error(ErrorOutput {
                ename: "RuntimeError".to_string(),
                evalue: "something went wrong".to_string(),
                traceback: vec!["tb line".to_string()],
            })],
        };
        let mut results_meta = blank_cell_metadata();
        results_meta.additional.insert(
            "nota-bene".to_string(),
            json!({"runner": {"role": "results"}}),
        );
        // Results cell ran but printed nothing useful (no marker)
        let results_cell = Cell::Code {
            id: cid("res"),
            metadata: results_meta,
            execution_count: None,
            source: vec![],
            outputs: vec![],
        };
        let nb = notebook_from(vec![test_cell, results_cell]);
        let result = extract_results(&nb, "c1", "my test");
        match result {
            CellTestResult::Error { error, .. } => {
                assert_eq!(error.phase, "test");
                assert!(error.detail.contains("RuntimeError"));
            }
            CellTestResult::Completed { .. } => panic!("expected Error"),
        }
    }

    /// Results cell is present but produced no __NB_RESULTS__ output and no
    /// other cell has an error → "Results cell produced no output".
    #[test]
    fn results_cell_present_no_marker_no_error() {
        let mut results_meta = blank_cell_metadata();
        results_meta.additional.insert(
            "nota-bene".to_string(),
            json!({"runner": {"role": "results"}}),
        );
        let results_cell = Cell::Code {
            id: cid("res"),
            metadata: results_meta,
            execution_count: None,
            source: vec![],
            outputs: vec![],
        };
        let nb = notebook_from(vec![results_cell]);
        let result = extract_results(&nb, "c1", "my test");
        match result {
            CellTestResult::Error { error, .. } => {
                assert_eq!(error.phase, "results");
                assert!(error.detail.contains("no output"));
            }
            CellTestResult::Completed { .. } => panic!("expected Error"),
        }
    }

    /// Results cell is entirely absent and a cell error exists → that error
    /// is surfaced.
    #[test]
    fn results_cell_missing_with_cell_error() {
        let mut cell_meta = blank_cell_metadata();
        cell_meta.additional.insert(
            "nota-bene".to_string(),
            json!({"runner": {"role": "cell_source", "source_cell_id": "c1"}}),
        );
        // Note: this is NOT a fixture/cell_source error — it's on a cell_source
        // role but we're not using find_setup_error here because the results
        // cell is absent. Use an unknown role so it bypasses find_setup_error.
        let mut other_meta = blank_cell_metadata();
        other_meta.additional.insert(
            "nota-bene".to_string(),
            json!({"runner": {"role": "unknown_role"}}),
        );
        let erroring_cell = Cell::Code {
            id: cid("other"),
            metadata: other_meta,
            execution_count: None,
            source: vec![],
            outputs: vec![Output::Error(ErrorOutput {
                ename: "KeyboardInterrupt".to_string(),
                evalue: "".to_string(),
                traceback: vec![],
            })],
        };
        // No results cell at all
        let nb = notebook_from(vec![erroring_cell]);
        let result = extract_results(&nb, "c1", "my test");
        match result {
            CellTestResult::Error { error, .. } => {
                assert!(error.detail.contains("KeyboardInterrupt"));
            }
            CellTestResult::Completed { .. } => panic!("expected Error"),
        }
    }

    /// Results cell is entirely absent and no cell errors exist →
    /// "Results cell missing from executed notebook".
    #[test]
    fn results_cell_missing_no_error() {
        // Notebook with only a plain code cell — no results cell
        let nb = notebook_from(vec![code_cell("c1", "x = 1")]);
        let result = extract_results(&nb, "c1", "my test");
        match result {
            CellTestResult::Error { error, .. } => {
                assert_eq!(error.phase, "executor");
                assert!(error.detail.contains("missing"));
            }
            CellTestResult::Completed { .. } => panic!("expected Error"),
        }
    }

    /// Results JSON is present but malformed → parse error reported.
    #[test]
    fn results_json_parse_failure() {
        let nb = make_executed_nb_with_results("not valid json {{");
        let result = extract_results(&nb, "c1", "my test");
        match result {
            CellTestResult::Error { error, .. } => {
                assert_eq!(error.phase, "results");
                assert!(error.detail.contains("Failed to parse results JSON"));
            }
            CellTestResult::Completed { .. } => panic!("expected Error"),
        }
    }
}
