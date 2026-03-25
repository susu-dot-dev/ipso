/// `nb update` — parse, validate, and apply JSON update blobs to notebook cells.
use anyhow::{Context, Result};
use nbformat::v4::Notebook;
use serde_json::Value;

use crate::diagnostics::{Diagnostic, DiagnosticType, Severity};
use crate::metadata::{Fixture, TestMeta};
use crate::notebook::{find_code_cell, CellExt};

/// Represents a field that distinguishes absent vs null vs value.
#[derive(Debug)]
pub enum UpdateField {
    /// Key was not present in the JSON object.
    Absent,
    /// Key was present with value `null`.
    Null,
    /// Key was present with a non-null value.
    Value(Value),
}

/// A single cell update blob as received from the user.
#[derive(Debug)]
pub struct CellUpdate {
    pub cell_id: String,
    pub fixtures: UpdateField,
    pub diff: UpdateField,
    pub test: UpdateField,
}

impl CellUpdate {
    fn from_value(v: &Value) -> Result<Self> {
        let obj = v
            .as_object()
            .ok_or_else(|| anyhow::anyhow!("update must be a JSON object"))?;
        let cell_id = obj
            .get("cell_id")
            .and_then(|v| v.as_str())
            .ok_or_else(|| {
                anyhow::anyhow!("update object missing required string field 'cell_id'")
            })?
            .to_string();

        fn field(obj: &serde_json::Map<String, Value>, key: &str) -> UpdateField {
            match obj.get(key) {
                None => UpdateField::Absent,
                Some(Value::Null) => UpdateField::Null,
                Some(v) => UpdateField::Value(v.clone()),
            }
        }

        Ok(CellUpdate {
            cell_id,
            fixtures: field(obj, "fixtures"),
            diff: field(obj, "diff"),
            test: field(obj, "test"),
        })
    }
}

/// Parse a JSON string into a vec of CellUpdate. Accepts a single object or array.
pub fn parse_updates(json_str: &str) -> Result<Vec<CellUpdate>> {
    let value: Value = serde_json::from_str(json_str).context("parsing update JSON")?;
    match &value {
        Value::Array(arr) => arr.iter().map(CellUpdate::from_value).collect(),
        Value::Object(_) => Ok(vec![CellUpdate::from_value(&value)?]),
        _ => anyhow::bail!("update data must be a JSON object or array"),
    }
}

/// Validate all updates against a notebook. Returns diagnostics for any errors.
pub fn validate_updates(updates: &[CellUpdate], nb: &Notebook) -> Vec<Diagnostic> {
    let mut diagnostics = Vec::new();

    for update in updates {
        // Check cell_id exists
        let cell_exists = find_code_cell(nb, &update.cell_id).is_some();
        if !cell_exists {
            diagnostics.push(Diagnostic {
                r#type: DiagnosticType::InvalidField,
                severity: Severity::Error,
                message: format!(
                    "No code cell with id '{}' exists in this notebook.",
                    update.cell_id
                ),
                field: "cell_id".to_string(),
            });
            continue;
        }

        // Validate fixtures
        if let UpdateField::Value(fixtures_val) = &update.fixtures {
            if let Some(obj) = fixtures_val.as_object() {
                for (key, val) in obj {
                    if val.is_null() {
                        continue;
                    }
                    validate_fixture(key, val, &mut diagnostics);
                }
            } else {
                diagnostics.push(Diagnostic {
                    r#type: DiagnosticType::InvalidField,
                    severity: Severity::Error,
                    message: "fixtures must be null (to clear) or an object mapping fixture names to fixture definitions.".to_string(),
                    field: "fixtures".to_string(),
                });
            }
        }

        // Validate test
        if let UpdateField::Value(test_val) = &update.test {
            if let Some(obj) = test_val.as_object() {
                if !obj.contains_key("name") {
                    diagnostics.push(Diagnostic {
                        r#type: DiagnosticType::InvalidField,
                        severity: Severity::Error,
                        message: "test is missing required field 'name'. Provide a test function name as a string.".to_string(),
                        field: "test.name".to_string(),
                    });
                } else if !obj["name"].is_string() {
                    diagnostics.push(Diagnostic {
                        r#type: DiagnosticType::InvalidField,
                        severity: Severity::Error,
                        message: "test.name must be a string (the name of the test function)."
                            .to_string(),
                        field: "test.name".to_string(),
                    });
                }
                if !obj.contains_key("source") {
                    diagnostics.push(Diagnostic {
                        r#type: DiagnosticType::InvalidField,
                        severity: Severity::Error,
                        message: "test is missing required field 'source'. Provide the test code as a string.".to_string(),
                        field: "test.source".to_string(),
                    });
                } else if !obj["source"].is_string() && !obj["source"].is_array() {
                    diagnostics.push(Diagnostic {
                        r#type: DiagnosticType::InvalidField,
                        severity: Severity::Error,
                        message: "test.source must be a string or array of strings containing the test code.".to_string(),
                        field: "test.source".to_string(),
                    });
                }
            } else {
                diagnostics.push(Diagnostic {
                    r#type: DiagnosticType::InvalidField,
                    severity: Severity::Error,
                    message:
                        "test must be null (to clear) or an object with 'name' and 'source' fields."
                            .to_string(),
                    field: "test".to_string(),
                });
            }
        }

        // Validate diff
        if let UpdateField::Value(diff_val) = &update.diff {
            if !diff_val.is_string() {
                diagnostics.push(Diagnostic {
                    r#type: DiagnosticType::InvalidField,
                    severity: Severity::Error,
                    message: "diff must be null (to clear) or a unified diff string.".to_string(),
                    field: "diff".to_string(),
                });
            }
        }
    }

    diagnostics
}

fn validate_fixture(key: &str, val: &Value, diagnostics: &mut Vec<Diagnostic>) {
    let Some(obj) = val.as_object() else {
        diagnostics.push(Diagnostic {
            r#type: DiagnosticType::InvalidField,
            severity: Severity::Error,
            message: format!("fixture '{key}' must be an object with 'description', 'priority', and 'source' fields, or null to remove it."),
            field: format!("fixtures.{key}"),
        });
        return;
    };

    if !obj.contains_key("description") {
        diagnostics.push(Diagnostic {
            r#type: DiagnosticType::InvalidField,
            severity: Severity::Error,
            message: format!("fixture '{key}' is missing required field 'description'. Provide a short description of what this fixture sets up."),
            field: format!("fixtures.{key}.description"),
        });
    } else if !obj["description"].is_string() {
        diagnostics.push(Diagnostic {
            r#type: DiagnosticType::InvalidField,
            severity: Severity::Error,
            message: format!("fixture '{key}' description must be a string."),
            field: format!("fixtures.{key}.description"),
        });
    }

    if !obj.contains_key("priority") {
        diagnostics.push(Diagnostic {
            r#type: DiagnosticType::InvalidField,
            severity: Severity::Error,
            message: format!("fixture '{key}' is missing required field 'priority'. Use an integer to control the order fixtures are applied (lower runs first)."),
            field: format!("fixtures.{key}.priority"),
        });
    } else if !obj["priority"].is_i64() {
        diagnostics.push(Diagnostic {
            r#type: DiagnosticType::InvalidField,
            severity: Severity::Error,
            message: format!("fixture '{key}' priority must be an integer (e.g. 0, 1, 10)."),
            field: format!("fixtures.{key}.priority"),
        });
    }

    if !obj.contains_key("source") {
        diagnostics.push(Diagnostic {
            r#type: DiagnosticType::InvalidField,
            severity: Severity::Error,
            message: format!("fixture '{key}' is missing required field 'source'. Provide the setup code as a string."),
            field: format!("fixtures.{key}.source"),
        });
    } else if !obj["source"].is_string() && !obj["source"].is_array() {
        diagnostics.push(Diagnostic {
            r#type: DiagnosticType::InvalidField,
            severity: Severity::Error,
            message: format!("fixture '{key}' source must be a string or array of strings containing the setup code."),
            field: format!("fixtures.{key}.source"),
        });
    }
}

/// Apply validated updates to the notebook.
pub fn apply_updates(updates: Vec<CellUpdate>, nb: &mut Notebook) -> Result<()> {
    for update in updates {
        let idx = find_code_cell(nb, &update.cell_id)
            .map(|(i, _)| i)
            .expect("cell existence was validated");
        let cell = &mut nb.cells[idx];

        // Apply fixtures
        match update.fixtures {
            UpdateField::Absent => {}
            UpdateField::Null => {
                cell.ipso_mut().set_fixtures(None);
            }
            UpdateField::Value(fixtures_val) => {
                // Merge semantics: read existing, merge, write back
                let existing = cell.ipso().and_then(|d| d.fixtures).unwrap_or_default();
                let mut merged = existing;

                let update_map = fixtures_val.as_object().expect("validated as object");
                for (key, val) in update_map {
                    if val.is_null() {
                        merged.shift_remove(key);
                    } else {
                        let fixture: Fixture = serde_json::from_value(val.clone())
                            .with_context(|| format!("parsing fixture '{key}'"))?;
                        merged.insert(key.clone(), fixture);
                    }
                }

                if merged.is_empty() {
                    cell.ipso_mut().set_fixtures(None);
                } else {
                    cell.ipso_mut().set_fixtures(Some(merged));
                }
            }
        }

        let mut view = cell.ipso_mut();

        // Apply test
        match update.test {
            UpdateField::Absent => {}
            UpdateField::Null => {
                view.set_test(None);
            }
            UpdateField::Value(test_val) => {
                let test: TestMeta = serde_json::from_value(test_val).context("parsing test")?;
                view.set_test(Some(test));
            }
        }

        // Apply diff
        match update.diff {
            UpdateField::Absent => {}
            UpdateField::Null => {
                view.set_diff(None);
            }
            UpdateField::Value(diff_val) => {
                view.set_diff(Some(
                    diff_val.as_str().expect("validated as string").to_string(),
                ));
            }
        }

        // Ensure ipso key exists
        view.mark_addressed();
    }

    Ok(())
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

    // ---- parse_updates ----

    #[test]
    fn parse_single_object() {
        let updates = parse_updates(r#"{"cell_id": "c1"}"#).unwrap();
        assert_eq!(updates.len(), 1);
        assert_eq!(updates[0].cell_id, "c1");
    }

    #[test]
    fn parse_array() {
        let updates = parse_updates(r#"[{"cell_id": "c1"}, {"cell_id": "c2"}]"#).unwrap();
        assert_eq!(updates.len(), 2);
        assert_eq!(updates[1].cell_id, "c2");
    }

    #[test]
    fn parse_empty_array() {
        let updates = parse_updates("[]").unwrap();
        assert!(updates.is_empty());
    }

    #[test]
    fn parse_non_object_non_array_rejected() {
        assert!(parse_updates(r#""hello""#).is_err());
        assert!(parse_updates("42").is_err());
        assert!(parse_updates("true").is_err());
    }

    #[test]
    fn parse_malformed_json_rejected() {
        assert!(parse_updates("{not json}").is_err());
    }

    #[test]
    fn parse_missing_cell_id_rejected() {
        assert!(parse_updates(r#"{"test": null}"#).is_err());
    }

    #[test]
    fn parse_non_string_cell_id_rejected() {
        assert!(parse_updates(r#"{"cell_id": 42}"#).is_err());
    }

    // ---- CellUpdate::from_value / UpdateField ----

    #[test]
    fn from_value_absent_fields() {
        let v = json!({"cell_id": "c1"});
        let u = CellUpdate::from_value(&v).unwrap();
        assert!(matches!(u.fixtures, UpdateField::Absent));
        assert!(matches!(u.diff, UpdateField::Absent));
        assert!(matches!(u.test, UpdateField::Absent));
    }

    #[test]
    fn from_value_null_fields() {
        let v = json!({"cell_id": "c1", "fixtures": null, "diff": null, "test": null});
        let u = CellUpdate::from_value(&v).unwrap();
        assert!(matches!(u.fixtures, UpdateField::Null));
        assert!(matches!(u.diff, UpdateField::Null));
        assert!(matches!(u.test, UpdateField::Null));
    }

    #[test]
    fn from_value_present_fields() {
        let v = json!({
            "cell_id": "c1",
            "fixtures": {"f1": {"description": "d", "priority": 1, "source": "s"}},
            "diff": "some diff",
            "test": {"name": "t", "source": "s"}
        });
        let u = CellUpdate::from_value(&v).unwrap();
        assert!(matches!(u.fixtures, UpdateField::Value(_)));
        assert!(matches!(u.diff, UpdateField::Value(_)));
        assert!(matches!(u.test, UpdateField::Value(_)));
    }

    // ---- validate_updates ----

    #[test]
    fn validate_unknown_cell() {
        let nb = notebook(vec![plain_cell("c1", "x = 1")]);
        let updates = parse_updates(r#"{"cell_id": "nonexistent"}"#).unwrap();
        let diags = validate_updates(&updates, &nb);
        assert_eq!(diags.len(), 1);
        assert_eq!(diags[0].r#type, DiagnosticType::InvalidField);
        assert_eq!(diags[0].severity, Severity::Error);
    }

    #[test]
    fn validate_fixtures_not_object() {
        let nb = notebook(vec![plain_cell("c1", "x = 1")]);
        let updates = parse_updates(r#"{"cell_id": "c1", "fixtures": 42}"#).unwrap();
        let diags = validate_updates(&updates, &nb);
        assert!(diags
            .iter()
            .any(|d| d.r#type == DiagnosticType::InvalidField && d.field == "fixtures"));
    }

    #[test]
    fn validate_fixture_missing_required_fields() {
        let nb = notebook(vec![plain_cell("c1", "x = 1")]);
        let updates =
            parse_updates(r#"{"cell_id": "c1", "fixtures": {"f1": {"description": "d"}}}"#)
                .unwrap();
        let diags = validate_updates(&updates, &nb);
        let fields: Vec<&str> = diags.iter().map(|d| d.field.as_str()).collect();
        assert!(fields.contains(&"fixtures.f1.priority"));
        assert!(fields.contains(&"fixtures.f1.source"));
    }

    #[test]
    fn validate_fixture_wrong_types() {
        let nb = notebook(vec![plain_cell("c1", "x = 1")]);
        let updates = parse_updates(
            r#"{"cell_id": "c1", "fixtures": {"f1": {"description": 42, "priority": "bad", "source": 99}}}"#,
        )
        .unwrap();
        let diags = validate_updates(&updates, &nb);
        assert_eq!(diags.len(), 3);
        assert!(diags
            .iter()
            .all(|d| d.r#type == DiagnosticType::InvalidField));
    }

    #[test]
    fn validate_fixture_null_entry_skipped() {
        let nb = notebook(vec![plain_cell("c1", "x = 1")]);
        let updates =
            parse_updates(r#"{"cell_id": "c1", "fixtures": {"remove_me": null}}"#).unwrap();
        let diags = validate_updates(&updates, &nb);
        assert!(diags.is_empty());
    }

    #[test]
    fn validate_fixture_not_object_entry() {
        let nb = notebook(vec![plain_cell("c1", "x = 1")]);
        let updates = parse_updates(r#"{"cell_id": "c1", "fixtures": {"f1": "bad"}}"#).unwrap();
        let diags = validate_updates(&updates, &nb);
        assert!(diags
            .iter()
            .any(|d| d.r#type == DiagnosticType::InvalidField && d.field == "fixtures.f1"));
    }

    #[test]
    fn validate_test_missing_name() {
        let nb = notebook(vec![plain_cell("c1", "x = 1")]);
        let updates = parse_updates(r#"{"cell_id": "c1", "test": {"source": "s"}}"#).unwrap();
        let diags = validate_updates(&updates, &nb);
        assert!(diags
            .iter()
            .any(|d| d.r#type == DiagnosticType::InvalidField && d.field == "test.name"));
    }

    #[test]
    fn validate_test_missing_source() {
        let nb = notebook(vec![plain_cell("c1", "x = 1")]);
        let updates = parse_updates(r#"{"cell_id": "c1", "test": {"name": "t"}}"#).unwrap();
        let diags = validate_updates(&updates, &nb);
        assert!(diags
            .iter()
            .any(|d| d.r#type == DiagnosticType::InvalidField && d.field == "test.source"));
    }

    #[test]
    fn validate_test_wrong_name_type() {
        let nb = notebook(vec![plain_cell("c1", "x = 1")]);
        let updates =
            parse_updates(r#"{"cell_id": "c1", "test": {"name": 42, "source": "s"}}"#).unwrap();
        let diags = validate_updates(&updates, &nb);
        assert!(diags
            .iter()
            .any(|d| d.r#type == DiagnosticType::InvalidField && d.field == "test.name"));
    }

    #[test]
    fn validate_test_wrong_source_type() {
        let nb = notebook(vec![plain_cell("c1", "x = 1")]);
        let updates =
            parse_updates(r#"{"cell_id": "c1", "test": {"name": "t", "source": 42}}"#).unwrap();
        let diags = validate_updates(&updates, &nb);
        assert!(diags
            .iter()
            .any(|d| d.r#type == DiagnosticType::InvalidField && d.field == "test.source"));
    }

    #[test]
    fn validate_test_not_object() {
        let nb = notebook(vec![plain_cell("c1", "x = 1")]);
        let updates = parse_updates(r#"{"cell_id": "c1", "test": "bad"}"#).unwrap();
        let diags = validate_updates(&updates, &nb);
        assert!(diags
            .iter()
            .any(|d| d.r#type == DiagnosticType::InvalidField && d.field == "test"));
    }

    #[test]
    fn validate_diff_wrong_type() {
        let nb = notebook(vec![plain_cell("c1", "x = 1")]);
        let updates = parse_updates(r#"{"cell_id": "c1", "diff": 42}"#).unwrap();
        let diags = validate_updates(&updates, &nb);
        assert!(diags
            .iter()
            .any(|d| d.r#type == DiagnosticType::InvalidField && d.field == "diff"));
    }

    #[test]
    fn validate_valid_update_no_diagnostics() {
        let nb = notebook(vec![plain_cell("c1", "x = 1")]);
        let updates = parse_updates(
            r#"{
            "cell_id": "c1",
            "test": {"name": "t", "source": "s"},
            "diff": "d",
            "fixtures": {"f": {"description": "d", "priority": 1, "source": "s"}}
        }"#,
        )
        .unwrap();
        let diags = validate_updates(&updates, &nb);
        assert!(diags.is_empty());
    }

    // ---- apply_updates ----

    #[test]
    fn apply_absent_fields_no_op() {
        let cell = cell_with_nb(
            "c1",
            "x = 1",
            json!({
                "test": {"name": "t", "source": "s"},
                "diff": "d"
            }),
        );
        let mut nb = notebook(vec![cell]);
        let updates = parse_updates(r#"{"cell_id": "c1"}"#).unwrap();
        apply_updates(updates, &mut nb).unwrap();
        let data = nb.cells[0].ipso().unwrap();
        assert!(data.test.is_some(), "test should be preserved");
        assert!(data.diff.is_some(), "diff should be preserved");
    }

    #[test]
    fn apply_null_clears_fields() {
        let cell = cell_with_nb(
            "c1",
            "x = 1",
            json!({
                "test": {"name": "t", "source": "s"},
                "diff": "d",
                "fixtures": {"f": {"description": "d", "priority": 1, "source": "s"}}
            }),
        );
        let mut nb = notebook(vec![cell]);
        let updates =
            parse_updates(r#"{"cell_id": "c1", "test": null, "diff": null, "fixtures": null}"#)
                .unwrap();
        apply_updates(updates, &mut nb).unwrap();
        let data = nb.cells[0].ipso().unwrap();
        assert!(data.test.is_none());
        assert!(data.diff.is_none());
        assert!(data.fixtures.is_none());
    }

    #[test]
    fn apply_set_test() {
        let mut nb = notebook(vec![plain_cell("c1", "x = 1")]);
        let updates =
            parse_updates(r#"{"cell_id": "c1", "test": {"name": "t1", "source": "assert True"}}"#)
                .unwrap();
        apply_updates(updates, &mut nb).unwrap();
        let data = nb.cells[0].ipso().unwrap();
        let test = data.test.unwrap();
        assert_eq!(test.name, "t1");
        assert_eq!(test.source, "assert True");
    }

    #[test]
    fn apply_set_diff() {
        let mut nb = notebook(vec![plain_cell("c1", "x = 1")]);
        let updates = parse_updates(r#"{"cell_id": "c1", "diff": "some diff"}"#).unwrap();
        apply_updates(updates, &mut nb).unwrap();
        let data = nb.cells[0].ipso().unwrap();
        assert_eq!(data.diff.unwrap(), "some diff");
    }

    #[test]
    fn apply_fixture_merge_upsert() {
        let cell = cell_with_nb(
            "c1",
            "x = 1",
            json!({
                "fixtures": {
                    "existing": {"description": "old", "priority": 0, "source": "old_src"}
                }
            }),
        );
        let mut nb = notebook(vec![cell]);
        let updates = parse_updates(
            r#"{
            "cell_id": "c1",
            "fixtures": {
                "new_fix": {"description": "new", "priority": 1, "source": "new_src"}
            }
        }"#,
        )
        .unwrap();
        apply_updates(updates, &mut nb).unwrap();
        let data = nb.cells[0].ipso().unwrap();
        let fixtures = data.fixtures.unwrap();
        assert!(
            fixtures.contains_key("existing"),
            "existing fixture preserved"
        );
        assert!(fixtures.contains_key("new_fix"), "new fixture added");
    }

    #[test]
    fn apply_fixture_merge_update_existing() {
        let cell = cell_with_nb(
            "c1",
            "x = 1",
            json!({
                "fixtures": {
                    "f1": {"description": "old", "priority": 0, "source": "old"}
                }
            }),
        );
        let mut nb = notebook(vec![cell]);
        let updates = parse_updates(
            r#"{
            "cell_id": "c1",
            "fixtures": {
                "f1": {"description": "updated", "priority": 5, "source": "new"}
            }
        }"#,
        )
        .unwrap();
        apply_updates(updates, &mut nb).unwrap();
        let data = nb.cells[0].ipso().unwrap();
        let f1 = &data.fixtures.unwrap()["f1"];
        assert_eq!(f1.description, "updated");
        assert_eq!(f1.priority, 5);
    }

    #[test]
    fn apply_fixture_remove_via_null() {
        let cell = cell_with_nb(
            "c1",
            "x = 1",
            json!({
                "fixtures": {
                    "f1": {"description": "d", "priority": 0, "source": "s"},
                    "f2": {"description": "d", "priority": 0, "source": "s"}
                }
            }),
        );
        let mut nb = notebook(vec![cell]);
        let updates = parse_updates(
            r#"{
            "cell_id": "c1",
            "fixtures": {"f1": null}
        }"#,
        )
        .unwrap();
        apply_updates(updates, &mut nb).unwrap();
        let data = nb.cells[0].ipso().unwrap();
        let fixtures = data.fixtures.unwrap();
        assert!(!fixtures.contains_key("f1"), "f1 should be removed");
        assert!(fixtures.contains_key("f2"), "f2 should remain");
    }

    #[test]
    fn apply_removing_all_fixtures_yields_none() {
        let cell = cell_with_nb(
            "c1",
            "x = 1",
            json!({
                "fixtures": {
                    "f1": {"description": "d", "priority": 0, "source": "s"}
                }
            }),
        );
        let mut nb = notebook(vec![cell]);
        let updates = parse_updates(
            r#"{
            "cell_id": "c1",
            "fixtures": {"f1": null}
        }"#,
        )
        .unwrap();
        apply_updates(updates, &mut nb).unwrap();
        let data = nb.cells[0].ipso().unwrap();
        assert!(
            data.fixtures.is_none(),
            "fixtures should be None when all removed"
        );
    }

    #[test]
    fn apply_ensures_nb_key_exists() {
        let mut nb = notebook(vec![plain_cell("c1", "x = 1")]);
        let updates = parse_updates(r#"{"cell_id": "c1"}"#).unwrap();
        apply_updates(updates, &mut nb).unwrap();
        assert!(nb.cells[0].ipso().is_some(), "ipso key should exist");
    }
}
