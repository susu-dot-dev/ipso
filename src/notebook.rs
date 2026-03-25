use anyhow::{Context, Result};
use nbformat::v4::{Cell, CellId, CellMetadata, Notebook};
use serde_json::Value;
use std::collections::HashMap;
use std::path::Path;

use crate::metadata::{read_ipso, IpsoData, IpsoView};

pub trait CellExt {
    fn cell_id(&self) -> &CellId;
    fn cell_id_str(&self) -> &str;
    fn source_str(&self) -> String;
    fn additional(&self) -> &HashMap<String, Value>;
    fn additional_mut(&mut self) -> &mut HashMap<String, Value>;
    fn ipso(&self) -> Option<IpsoData>;
    fn ipso_mut(&mut self) -> IpsoView<'_>;
    fn editor_role(&self) -> Option<String>;
    fn editor_cell_id(&self) -> Option<String>;
}

impl CellExt for Cell {
    fn cell_id(&self) -> &CellId {
        match self {
            Cell::Code { id, .. } => id,
            Cell::Markdown { id, .. } => id,
            Cell::Raw { id, .. } => id,
        }
    }

    fn cell_id_str(&self) -> &str {
        self.cell_id().as_str()
    }

    fn source_str(&self) -> String {
        match self {
            Cell::Code { source, .. } => source.join(""),
            Cell::Markdown { source, .. } => source.join(""),
            Cell::Raw { source, .. } => source.join(""),
        }
    }

    fn additional(&self) -> &HashMap<String, Value> {
        match self {
            Cell::Code { metadata, .. } => &metadata.additional,
            Cell::Markdown { metadata, .. } => &metadata.additional,
            Cell::Raw { metadata, .. } => &metadata.additional,
        }
    }

    fn additional_mut(&mut self) -> &mut HashMap<String, Value> {
        match self {
            Cell::Code { metadata, .. } => &mut metadata.additional,
            Cell::Markdown { metadata, .. } => &mut metadata.additional,
            Cell::Raw { metadata, .. } => &mut metadata.additional,
        }
    }

    fn ipso(&self) -> Option<IpsoData> {
        read_ipso(self.additional())
    }

    fn ipso_mut(&mut self) -> IpsoView<'_> {
        IpsoView::new(self.additional_mut())
    }

    fn editor_role(&self) -> Option<String> {
        self.additional()
            .get("ipso")
            .and_then(|v| v.get("editor"))
            .and_then(|e| e.get("role"))
            .and_then(|r| r.as_str())
            .map(|s| s.to_string())
    }

    fn editor_cell_id(&self) -> Option<String> {
        self.additional()
            .get("ipso")
            .and_then(|v| v.get("editor"))
            .and_then(|e| e.get("cell_id"))
            .and_then(|r| r.as_str())
            .map(|s| s.to_string())
    }
}

/// Parse a notebook from an already-loaded string.  Used for `--stdin` mode.
///
/// Only nbformat 4.5 notebooks are accepted.  Older notebooks must be
/// upgraded first with `ipso upgrade`.
pub fn load_notebook_from_str(content: &str, path_hint: &str) -> Result<Notebook> {
    let versioned = nbformat::parse_notebook(content)
        .with_context(|| format!("parsing notebook {path_hint}"))?;
    match versioned {
        nbformat::Notebook::V4(nb) => Ok(nb),
        nbformat::Notebook::Legacy(_) | nbformat::Notebook::V3(_) => {
            anyhow::bail!(
                "notebook \"{path_hint}\" is not nbformat 4.5.\n\
                 Run `ipso upgrade {path_hint}` to add stable cell IDs."
            )
        }
    }
}

/// Load a notebook from a file path.
///
/// Only nbformat 4.5 notebooks are accepted.  Older notebooks must be
/// upgraded first with `ipso upgrade`.
pub fn load_notebook(path: &Path) -> Result<Notebook> {
    let content = std::fs::read_to_string(path)
        .with_context(|| format!("reading notebook {}", path.display()))?;
    let path_hint = path.display().to_string();
    let versioned = nbformat::parse_notebook(&content)
        .with_context(|| format!("parsing notebook {path_hint}"))?;
    match versioned {
        nbformat::Notebook::V4(nb) => Ok(nb),
        nbformat::Notebook::Legacy(_) | nbformat::Notebook::V3(_) => {
            anyhow::bail!(
                "notebook \"{path_hint}\" is not nbformat 4.5.\n\
                 Run `ipso upgrade {path_hint}` to add stable cell IDs."
            )
        }
    }
}

pub fn save_notebook(nb: &Notebook, path: &Path) -> Result<()> {
    let json = nbformat::serialize_notebook(&nbformat::Notebook::V4(nb.clone()))
        .with_context(|| format!("serializing notebook {}", path.display()))?;
    std::fs::write(path, json).with_context(|| format!("writing notebook {}", path.display()))?;
    Ok(())
}

/// Remove keys with null values from `metadata.language_info` in serialised
/// notebook JSON. This works around a limitation in the nbformat crate where
/// `Option::None` fields are serialised as `null` rather than being omitted.
#[allow(dead_code)]
fn strip_null_language_info_fields(json: &str) -> Result<String> {
    let mut value: serde_json::Value =
        serde_json::from_str(json).context("parsing serialized notebook JSON")?;
    if let Some(li) = value
        .get_mut("metadata")
        .and_then(|m| m.get_mut("language_info"))
        .and_then(|li| li.as_object_mut())
    {
        li.retain(|_, v| !v.is_null());
    }
    serde_json::to_string_pretty(&value).context("re-serializing notebook JSON")
}

/// Find the first code cell with the given `cell_id`, returning its index
/// and a reference to the cell.  Returns `None` if no code cell with that ID
/// exists.
pub fn find_code_cell<'a>(nb: &'a Notebook, cell_id: &str) -> Option<(usize, &'a Cell)> {
    nb.cells
        .iter()
        .enumerate()
        .find(|(_, cell)| matches!(cell, Cell::Code { .. }) && cell.cell_id_str() == cell_id)
}

pub fn blank_cell_metadata() -> CellMetadata {
    CellMetadata {
        id: None,
        collapsed: None,
        scrolled: None,
        deletable: None,
        editable: None,
        format: None,
        name: None,
        tags: None,
        jupyter: None,
        execution: None,
        additional: HashMap::new(),
    }
}

pub fn clear_editor_meta(cell: &mut Cell) {
    cell.ipso_mut().remove_field("editor");
}

/// Build a new random CellId.
pub fn new_cell_id() -> CellId {
    use uuid::Uuid;
    CellId::from(Uuid::new_v4())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn cid(s: &str) -> CellId {
        CellId::new(s).unwrap()
    }

    fn code_cell(id: &str, lines: Vec<&str>) -> Cell {
        Cell::Code {
            id: cid(id),
            metadata: blank_cell_metadata(),
            execution_count: None,
            source: lines.into_iter().map(String::from).collect(),
            outputs: vec![],
        }
    }

    fn code_cell_with_nb(id: &str, lines: Vec<&str>, nb_val: serde_json::Value) -> Cell {
        let mut meta = blank_cell_metadata();
        meta.additional.insert("ipso".to_string(), nb_val);
        Cell::Code {
            id: cid(id),
            metadata: meta,
            execution_count: None,
            source: lines.into_iter().map(String::from).collect(),
            outputs: vec![],
        }
    }

    // --- source_str ---

    #[test]
    fn source_str_empty_vec() {
        assert_eq!(code_cell("c1", vec![]).source_str(), "");
    }

    #[test]
    fn source_str_single_line() {
        assert_eq!(code_cell("c1", vec!["x = 1"]).source_str(), "x = 1");
    }

    #[test]
    fn source_str_multiline_joined() {
        assert_eq!(
            code_cell("c1", vec!["x = 1\n", "y = 2"]).source_str(),
            "x = 1\ny = 2"
        );
    }

    // --- cell_id / cell_id_str ---

    #[test]
    fn cell_id_str_returns_correct_id() {
        assert_eq!(code_cell("my-cell", vec![]).cell_id_str(), "my-cell");
    }

    // --- editor_role ---

    #[test]
    fn editor_role_present() {
        let cell = code_cell_with_nb("c1", vec![], json!({"editor": {"role": "setup"}}));
        assert_eq!(cell.editor_role(), Some("setup".to_string()));
    }

    #[test]
    fn editor_role_absent_when_no_nb_key() {
        assert_eq!(code_cell("c1", vec![]).editor_role(), None);
    }

    #[test]
    fn editor_role_absent_when_no_editor_key() {
        let cell = code_cell_with_nb("c1", vec![], json!({"diff": "d"}));
        assert_eq!(cell.editor_role(), None);
    }

    #[test]
    fn editor_role_absent_when_no_role_key() {
        let cell = code_cell_with_nb("c1", vec![], json!({"editor": {}}));
        assert_eq!(cell.editor_role(), None);
    }

    // --- editor_cell_id ---

    #[test]
    fn editor_cell_id_present() {
        let cell = code_cell_with_nb("c1", vec![], json!({"editor": {"cell_id": "other-cell"}}));
        assert_eq!(cell.editor_cell_id(), Some("other-cell".to_string()));
    }

    #[test]
    fn editor_cell_id_absent_when_no_nb_key() {
        assert_eq!(code_cell("c1", vec![]).editor_cell_id(), None);
    }

    #[test]
    fn editor_cell_id_absent_when_no_cell_id_key() {
        let cell = code_cell_with_nb("c1", vec![], json!({"editor": {"role": "test"}}));
        assert_eq!(cell.editor_cell_id(), None);
    }

    // --- ipso / ipso_mut ---

    #[test]
    fn ipso_absent_when_no_key() {
        assert!(code_cell("c1", vec![]).ipso().is_none());
    }

    #[test]
    fn ipso_present_when_key_exists() {
        let cell = code_cell_with_nb("c1", vec![], json!({"diff": "d"}));
        assert!(cell.ipso().is_some());
    }

    // --- clear_editor_meta ---

    #[test]
    fn clear_editor_meta_removes_editor_subkey() {
        let mut cell = code_cell_with_nb(
            "c1",
            vec![],
            json!({"editor": {"role": "test"}, "diff": "d"}),
        );
        clear_editor_meta(&mut cell);
        let data = cell.ipso().expect("expected Some");
        assert!(!data.extra.contains_key("editor"));
        // diff should still be there
        assert!(data.diff.is_some());
    }

    #[test]
    fn clear_editor_meta_no_op_when_no_nb_key() {
        let mut cell = code_cell("c1", vec![]);
        clear_editor_meta(&mut cell); // must not panic
    }

    // --- blank_cell_metadata ---

    #[test]
    fn blank_cell_metadata_is_empty() {
        let meta = blank_cell_metadata();
        assert!(meta.additional.is_empty());
        assert!(meta.collapsed.is_none());
        assert!(meta.tags.is_none());
    }

    // --- new_cell_id ---

    #[test]
    fn new_cell_id_produces_non_empty_string() {
        let id = new_cell_id();
        assert!(!id.as_str().is_empty());
    }

    #[test]
    fn new_cell_id_produces_unique_ids() {
        let a = new_cell_id();
        let b = new_cell_id();
        assert_ne!(a.as_str(), b.as_str());
    }
}
