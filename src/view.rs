/// `nb view` output types and serialization.
///
/// A [`CellView`] is the JSON representation of a single code cell as exposed
/// by `nb view`.  The spec says `shas` are never surfaced; instead cell state
/// information appears in `status.diagnostics`.
use std::collections::HashSet;

use indexmap::IndexMap;
use serde::Serialize;
use serde_json::{Map, Value};

use nbformat::v4::Notebook;

use crate::diagnostics::{compute_cell_diagnostics, CellStatus};
use crate::metadata::{Fixture, TestMeta};
use crate::notebook::CellExt;

/// JSON representation of a single code cell (view output).
///
/// Fields match the spec's "Cell object" schema.  `shas` is intentionally
/// absent — cell state is surfaced through `status`.
#[derive(Debug, Serialize)]
pub struct CellView {
    pub cell_id: String,
    pub source: String,
    /// `null` when no fixtures are stored.
    pub fixtures: Option<IndexMap<String, Fixture>>,
    /// `null` when no diff is stored.
    pub diff: Option<String>,
    /// `null` when no test is stored.
    pub test: Option<TestMeta>,
    pub status: CellStatus,
}

impl CellView {
    /// Build a `CellView` from a cell at position `index` in `nb`.
    ///
    /// `index` must be the position of `cell` in `nb.cells` so that
    /// `compute_cell_diagnostics` can compare SHAs against the full notebook.
    pub fn from_cell(nb: &Notebook, index: usize) -> Self {
        let cell = &nb.cells[index];
        let nb_data = cell.ipso().unwrap_or_default();
        let status = compute_cell_diagnostics(nb, index);

        CellView {
            cell_id: cell.cell_id_str().to_string(),
            source: cell.source_str(),
            fixtures: nb_data.fixtures,
            diff: nb_data.diff,
            test: nb_data.test,
            status,
        }
    }

    /// Serialize to a `serde_json::Value` and, if `fields` is non-empty,
    /// strip all top-level keys that are not in `fields` (keeping `cell_id`
    /// unconditionally).
    pub fn to_json_value(&self, fields: &Option<Vec<String>>) -> Value {
        let full: Value = serde_json::to_value(self).expect("CellView is always serializable");
        match fields {
            None => full,
            Some(wanted) => {
                let wanted_set: HashSet<&str> = wanted.iter().map(|s| s.as_str()).collect();
                let obj = full.as_object().expect("CellView serializes as object");
                let filtered: Map<String, Value> = obj
                    .iter()
                    .filter(|(k, _)| k.as_str() == "cell_id" || wanted_set.contains(k.as_str()))
                    .map(|(k, v)| (k.clone(), v.clone()))
                    .collect();
                Value::Object(filtered)
            }
        }
    }
}

/// Parse a `--fields f1,f2,...` string into a list of field names.
pub fn parse_fields(s: &str) -> Vec<String> {
    s.split(',').map(|f| f.trim().to_string()).collect()
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

    fn cell_with_nb(id: &str, source: &str, nb_val: serde_json::Value) -> Cell {
        let mut meta = blank_cell_metadata();
        meta.additional.insert("ipso".to_string(), nb_val);
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

    // --- CellView::from_cell ---

    #[test]
    fn from_plain_cell_has_no_fixtures_diff_test_and_is_missing() {
        // Plain code cells now produce a Missing diagnostic (bug fix: they must
        // be configured before they can be considered valid).
        let cell = plain_cell("c1", "x = 1");
        let nb = notebook(vec![cell]);
        let view = CellView::from_cell(&nb, 0);
        assert_eq!(view.cell_id, "c1");
        assert_eq!(view.source, "x = 1");
        assert!(view.fixtures.is_none());
        assert!(view.diff.is_none());
        assert!(view.test.is_none());
        assert!(!view.status.valid);
        assert!(view
            .status
            .diagnostics
            .iter()
            .any(|d| d.r#type == crate::diagnostics::DiagnosticType::Missing));
    }

    #[test]
    fn from_cell_with_test_includes_test() {
        let cell = cell_with_nb(
            "c1",
            "x = 1",
            json!({"test": {"name": "test_x", "source": "assert x == 1"}}),
        );
        let nb = notebook(vec![cell]);
        let view = CellView::from_cell(&nb, 0);
        assert!(view.test.is_some());
        assert_eq!(view.test.unwrap().name, "test_x");
    }

    #[test]
    fn from_cell_with_shas_valid_is_valid() {
        let raw = plain_cell("c1", "x = 1");
        let shas = json!([sha_json(&raw)]);
        let cell = cell_with_nb("c1", "x = 1", json!({"shas": shas}));
        let nb = notebook(vec![cell]);
        let view = CellView::from_cell(&nb, 0);
        assert!(view.status.valid);
    }

    #[test]
    fn shas_not_in_serialized_output() {
        let raw = plain_cell("c1", "x = 1");
        let shas = json!([sha_json(&raw)]);
        let cell = cell_with_nb("c1", "x = 1", json!({"shas": shas}));
        let nb = notebook(vec![cell]);
        let view = CellView::from_cell(&nb, 0);
        let val = view.to_json_value(&None);
        assert!(val.get("shas").is_none(), "shas must not appear in output");
    }

    // --- to_json_value with fields projection ---

    #[test]
    fn fields_projection_keeps_requested_fields() {
        let cell = plain_cell("c1", "x = 1");
        let nb = notebook(vec![cell]);
        let view = CellView::from_cell(&nb, 0);
        let val = view.to_json_value(&Some(vec!["source".to_string()]));
        assert!(val.get("cell_id").is_some());
        assert!(val.get("source").is_some());
        // Other fields stripped
        assert!(val.get("fixtures").is_none());
        assert!(val.get("status").is_none());
    }

    #[test]
    fn fields_none_returns_all_fields() {
        let cell = plain_cell("c1", "x = 1");
        let nb = notebook(vec![cell]);
        let view = CellView::from_cell(&nb, 0);
        let val = view.to_json_value(&None);
        assert!(val.get("cell_id").is_some());
        assert!(val.get("source").is_some());
        assert!(val.get("status").is_some());
    }

    #[test]
    fn cell_id_always_kept_even_when_not_in_fields() {
        let cell = plain_cell("c1", "x = 1");
        let nb = notebook(vec![cell]);
        let view = CellView::from_cell(&nb, 0);
        let val = view.to_json_value(&Some(vec!["source".to_string()]));
        assert!(val.get("cell_id").is_some());
    }

    // --- parse_fields ---

    #[test]
    fn parse_fields_single() {
        assert_eq!(parse_fields("status"), vec!["status"]);
    }

    #[test]
    fn parse_fields_multiple() {
        let fields = parse_fields("cell_id,source,test");
        assert_eq!(fields, vec!["cell_id", "source", "test"]);
    }

    #[test]
    fn parse_fields_trims_whitespace() {
        let fields = parse_fields("cell_id, source , test");
        assert_eq!(fields, vec!["cell_id", "source", "test"]);
    }

    // --- CellView::from_cell with various metadata ---

    #[test]
    fn from_cell_with_fixtures_includes_fixtures() {
        let cell = cell_with_nb(
            "c1",
            "x = 1",
            json!({"fixtures": {"f1": {"description": "d", "priority": 1, "source": "s"}}}),
        );
        let nb = notebook(vec![cell]);
        let view = CellView::from_cell(&nb, 0);
        let fixtures = view.fixtures.unwrap();
        assert!(fixtures.contains_key("f1"));
        assert_eq!(fixtures["f1"].description, "d");
    }

    #[test]
    fn from_cell_with_diff_includes_diff() {
        let cell = cell_with_nb("c1", "x = 1", json!({"diff": "some diff"}));
        let nb = notebook(vec![cell]);
        let view = CellView::from_cell(&nb, 0);
        assert_eq!(view.diff.unwrap(), "some diff");
    }

    #[test]
    fn from_cell_with_nb_no_shas_has_missing_sha_diagnostic() {
        let cell = cell_with_nb("c1", "x = 1", json!({}));
        let nb = notebook(vec![cell]);
        let view = CellView::from_cell(&nb, 0);
        assert!(!view.status.valid);
        assert!(view
            .status
            .diagnostics
            .iter()
            .any(|d| { d.r#type == crate::diagnostics::DiagnosticType::Missing }));
    }
}
