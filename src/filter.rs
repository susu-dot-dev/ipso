/// Filter parsing and evaluation for the `nb view` / `nb status` / `nb accept` commands.
///
/// Each `--filter "key:expr"` flag is parsed into a [`Filter`].  Multiple
/// filters combine with AND; comma-separated values within a single filter
/// combine with OR.
use anyhow::{bail, Result};
use nbformat::v4::{Cell, Notebook};

use crate::diagnostics::compute_cell_diagnostics;
use crate::notebook::CellExt;

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// A single parsed `--filter "key:expr"` argument.
#[derive(Debug, Clone)]
pub struct Filter {
    key: FilterKey,
    /// The comma-split OR-values from the expr part.
    values: Vec<String>,
}

impl Filter {
    /// Parse a raw `"key:expr"` string (the value of one `--filter` flag).
    pub fn parse(raw: &str) -> Result<Self> {
        let colon = raw
            .find(':')
            .ok_or_else(|| anyhow::anyhow!("filter must be in the form 'key:expr', got: {raw}"))?;
        let key_str = &raw[..colon];
        let expr = &raw[colon + 1..];
        let key = FilterKey::parse(key_str)?;
        let values: Vec<String> = expr.split(',').map(|s| s.trim().to_string()).collect();
        Ok(Filter { key, values })
    }

    /// Returns `true` if `cell` (at `index` in `nb`) matches this filter.
    ///
    /// Multiple values inside the filter are OR-ed.
    pub fn matches(&self, nb: &Notebook, cell: &Cell, index: usize) -> bool {
        self.values
            .iter()
            .any(|v| self.key.matches(nb, cell, index, v))
    }
}

/// Apply a slice of filters (AND semantics) to one cell.
pub fn cell_matches_all(filters: &[Filter], nb: &Notebook, cell: &Cell, index: usize) -> bool {
    filters.iter().all(|f| f.matches(nb, cell, index))
}

// ---------------------------------------------------------------------------
// Filter keys
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
enum FilterKey {
    /// `cell` — match by cell_id
    Cell,
    /// `index` — match by 0-based position (supports n, n..m, n.., ..m)
    Index,
    /// `test` — presence/absence
    Test,
    /// `fixtures` — presence/absence
    Fixtures,
    /// `diff` — presence/absence
    Diff,
    /// `status.valid` — true / false
    StatusValid,
    /// `diagnostics.type` — diagnostic type value
    DiagnosticsType,
    /// `diagnostics.severity` — error / warning
    DiagnosticsSeverity,
}

impl FilterKey {
    fn parse(s: &str) -> Result<Self> {
        match s {
            "cell" => Ok(FilterKey::Cell),
            "index" => Ok(FilterKey::Index),
            "test" => Ok(FilterKey::Test),
            "fixtures" => Ok(FilterKey::Fixtures),
            "diff" => Ok(FilterKey::Diff),
            "status.valid" => Ok(FilterKey::StatusValid),
            "diagnostics.type" => Ok(FilterKey::DiagnosticsType),
            "diagnostics.severity" => Ok(FilterKey::DiagnosticsSeverity),
            other => bail!("unknown filter key: {other}"),
        }
    }

    /// Whether `cell` satisfies `key = value` for this key variant.
    fn matches(&self, nb: &Notebook, cell: &Cell, index: usize, value: &str) -> bool {
        match self {
            FilterKey::Cell => cell.cell_id_str() == value,

            FilterKey::Index => match_index(index, value),

            FilterKey::Test => {
                let has_test = cell.nota_bene().and_then(|d| d.test).is_some();
                match_null_or_not(has_test, value)
            }

            FilterKey::Fixtures => {
                let has_fixtures = cell
                    .nota_bene()
                    .and_then(|d| d.fixtures)
                    .map(|f| !f.is_empty())
                    .unwrap_or(false);
                match_null_or_not(has_fixtures, value)
            }

            FilterKey::Diff => {
                let has_diff = cell.nota_bene().and_then(|d| d.diff).is_some();
                match_null_or_not(has_diff, value)
            }

            FilterKey::StatusValid => {
                let status = compute_cell_diagnostics(nb, index);
                let valid_str = if status.valid { "true" } else { "false" };
                valid_str == value
            }

            FilterKey::DiagnosticsType => {
                let status = compute_cell_diagnostics(nb, index);
                // value is a single type string (comma-split already happened in Filter::parse)
                status.diagnostics.iter().any(|d| {
                    let type_str = serde_json::to_value(&d.r#type)
                        .ok()
                        .and_then(|v| v.as_str().map(|s| s.to_string()))
                        .unwrap_or_default();
                    type_str == value
                })
            }

            FilterKey::DiagnosticsSeverity => {
                let status = compute_cell_diagnostics(nb, index);
                status.diagnostics.iter().any(|d| {
                    let sev_str = serde_json::to_value(&d.severity)
                        .ok()
                        .and_then(|v| v.as_str().map(|s| s.to_string()))
                        .unwrap_or_default();
                    sev_str == value
                })
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Match `"null"` / `"not null"` against a has-value boolean.
fn match_null_or_not(has_value: bool, expr: &str) -> bool {
    match expr {
        "null" => !has_value,
        "not null" => has_value,
        _ => false,
    }
}

/// Match index filter expressions: `n`, `n..m`, `n..`, `..m`.
fn match_index(index: usize, expr: &str) -> bool {
    if let Some(rest) = expr.strip_prefix("..") {
        // `..m` — indices 0..=m
        if let Ok(end) = rest.parse::<usize>() {
            return index <= end;
        }
    } else if expr.contains("..") {
        let parts: Vec<&str> = expr.splitn(2, "..").collect();
        let start = parts[0].parse::<usize>().unwrap_or(0);
        if parts[1].is_empty() {
            // `n..`
            return index >= start;
        } else if let Ok(end) = parts[1].parse::<usize>() {
            // `n..m`
            return index >= start && index <= end;
        }
    } else if let Ok(n) = expr.parse::<usize>() {
        return index == n;
    }
    false
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

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
        meta.additional.insert("nota-bene".to_string(), nb_val);
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
            metadata: nbformat::v4::Metadata::default(),
            nbformat: 4,
            nbformat_minor: 5,
            cells,
        }
    }

    fn sha_of(cell: &Cell) -> String {
        compute_cell_sha(cell)
    }

    // --- Filter::parse ---

    #[test]
    fn parse_simple_key_value() {
        let f = Filter::parse("cell:my-cell").unwrap();
        assert_eq!(f.values, vec!["my-cell"]);
    }

    #[test]
    fn parse_multi_value_or() {
        let f = Filter::parse("diagnostics.type:stale,diff_conflict").unwrap();
        assert_eq!(f.values, vec!["stale", "diff_conflict"]);
    }

    #[test]
    fn parse_unknown_key_errors() {
        assert!(Filter::parse("unknown_key:value").is_err());
    }

    #[test]
    fn parse_missing_colon_errors() {
        assert!(Filter::parse("nocoron").is_err());
    }

    // --- match_index ---

    #[test]
    fn index_exact() {
        assert!(match_index(3, "3"));
        assert!(!match_index(2, "3"));
    }

    #[test]
    fn index_range_n_to_m() {
        assert!(match_index(2, "1..3"));
        assert!(match_index(1, "1..3"));
        assert!(match_index(3, "1..3"));
        assert!(!match_index(0, "1..3"));
        assert!(!match_index(4, "1..3"));
    }

    #[test]
    fn index_range_open_end() {
        assert!(match_index(5, "2.."));
        assert!(match_index(2, "2.."));
        assert!(!match_index(1, "2.."));
    }

    #[test]
    fn index_range_open_start() {
        assert!(match_index(0, "..3"));
        assert!(match_index(3, "..3"));
        assert!(!match_index(4, "..3"));
    }

    // --- FilterKey::Cell ---

    #[test]
    fn filter_cell_id_matches() {
        let cell = plain_cell("my-id", "x = 1");
        let nb = notebook(vec![cell.clone()]);
        let f = Filter::parse("cell:my-id").unwrap();
        assert!(f.matches(&nb, &cell, 0));
    }

    #[test]
    fn filter_cell_id_no_match() {
        let cell = plain_cell("my-id", "x = 1");
        let nb = notebook(vec![cell.clone()]);
        let f = Filter::parse("cell:other-id").unwrap();
        assert!(!f.matches(&nb, &cell, 0));
    }

    // --- FilterKey::Test ---

    #[test]
    fn filter_test_null_when_absent() {
        let cell = plain_cell("c1", "x = 1");
        let nb = notebook(vec![cell.clone()]);
        let f = Filter::parse("test:null").unwrap();
        assert!(f.matches(&nb, &cell, 0));
    }

    #[test]
    fn filter_test_not_null_when_present() {
        let cell = cell_with_nb(
            "c1",
            "x = 1",
            json!({"test": {"name": "t", "source": "assert True"}}),
        );
        let nb = notebook(vec![cell.clone()]);
        let f = Filter::parse("test:not null").unwrap();
        assert!(f.matches(&nb, &cell, 0));
    }

    // --- FilterKey::StatusValid ---

    #[test]
    fn filter_status_valid_true_for_plain_cell() {
        let cell = plain_cell("c1", "x = 1");
        let nb = notebook(vec![cell.clone()]);
        let f = Filter::parse("status.valid:true").unwrap();
        assert!(f.matches(&nb, &cell, 0));
    }

    #[test]
    fn filter_status_valid_false_for_unaccepted_cell_with_nb() {
        let cell = cell_with_nb("c1", "x = 1", json!({}));
        let nb = notebook(vec![cell.clone()]);
        let f = Filter::parse("status.valid:false").unwrap();
        assert!(f.matches(&nb, &cell, 0));
    }

    // --- cell_matches_all (AND semantics) ---

    #[test]
    fn cell_matches_all_with_no_filters_is_true() {
        let cell = plain_cell("c1", "x = 1");
        let nb = notebook(vec![cell.clone()]);
        assert!(cell_matches_all(&[], &nb, &cell, 0));
    }

    #[test]
    fn cell_matches_all_and_semantics() {
        let cell = plain_cell("c1", "x = 1");
        let nb = notebook(vec![cell.clone()]);
        let f1 = Filter::parse("cell:c1").unwrap();
        let f2 = Filter::parse("test:null").unwrap();
        // Both match
        assert!(cell_matches_all(&[f1, f2], &nb, &cell, 0));
    }

    #[test]
    fn cell_matches_all_fails_if_one_filter_misses() {
        let cell = plain_cell("c1", "x = 1");
        let nb = notebook(vec![cell.clone()]);
        let f1 = Filter::parse("cell:c1").unwrap();
        let f2 = Filter::parse("cell:other").unwrap();
        assert!(!cell_matches_all(&[f1, f2], &nb, &cell, 0));
    }
}
