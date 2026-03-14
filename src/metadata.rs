use indexmap::IndexMap;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;

// ---------------------------------------------------------------------------
// Leaf types
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ShaEntry {
    pub cell_id: String,
    pub sha: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Fixture {
    pub description: String,
    pub priority: i64,
    #[serde(with = "source_lines")]
    pub source: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TestMeta {
    pub name: String,
    #[serde(with = "source_lines")]
    pub source: String,
}

// ---------------------------------------------------------------------------
// NotaBeneData  (owned snapshot, read-only)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct NotaBeneData {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fixtures: Option<IndexMap<String, Fixture>>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub diff: Option<String>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub test: Option<TestMeta>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shas: Option<Vec<ShaEntry>>,

    /// Catches unknown sub-keys (e.g. "editor") so they survive round-trips.
    #[serde(flatten)]
    pub extra: IndexMap<String, Value>,
}

// ---------------------------------------------------------------------------
// NotaBeneView  (mutable borrow for writing)
// ---------------------------------------------------------------------------

pub struct NotaBeneView<'a> {
    additional: &'a mut HashMap<String, Value>,
}

impl<'a> NotaBeneView<'a> {
    pub fn new(additional: &'a mut HashMap<String, Value>) -> Self {
        Self { additional }
    }

    /// Ensure `"nota-bene": {}` exists; return &mut Map.
    pub fn ensure_nb_object(&mut self) -> &mut serde_json::Map<String, Value> {
        let entry = self
            .additional
            .entry("nota-bene".to_string())
            .or_insert_with(|| Value::Object(Default::default()));
        entry.as_object_mut().expect("nota-bene must be an object")
    }

    fn set_field(&mut self, key: &str, value: Value) {
        self.ensure_nb_object().insert(key.to_string(), value);
    }

    pub fn remove_field(&mut self, key: &str) {
        if let Some(nb) = self.additional.get_mut("nota-bene") {
            if let Some(obj) = nb.as_object_mut() {
                obj.remove(key);
            }
        }
    }

    /// Ensure the "nota-bene" key exists (even if empty).
    pub fn mark_addressed(&mut self) {
        self.ensure_nb_object();
    }

    // ---- fixtures ----------------------------------------------------------

    /// `Some(map)` writes the fixtures object; `None` removes the fixtures key.
    pub fn set_fixtures(&mut self, v: Option<IndexMap<String, Fixture>>) {
        match v {
            Some(map) => {
                let json = serde_json::to_value(&map)
                    .expect("IndexMap<String, Fixture> is always serializable");
                self.set_field("fixtures", json);
            }
            None => self.remove_field("fixtures"),
        }
    }

    // ---- diff --------------------------------------------------------------

    /// `Some(s)` writes the diff string; `None` removes the diff key.
    pub fn set_diff(&mut self, v: Option<String>) {
        match v {
            Some(s) => self.set_field("diff", Value::String(s)),
            None => self.remove_field("diff"),
        }
    }

    // ---- test --------------------------------------------------------------

    /// `Some(t)` writes the test object; `None` removes the test key.
    pub fn set_test(&mut self, v: Option<TestMeta>) {
        match v {
            Some(t) => {
                let json = serde_json::to_value(&t).expect("TestMeta is always serializable");
                self.set_field("test", json);
            }
            None => self.remove_field("test"),
        }
    }

    // ---- shas --------------------------------------------------------------

    pub fn set_shas(&mut self, shas: Vec<ShaEntry>) {
        let json = serde_json::to_value(&shas).expect("Vec<ShaEntry> is always serializable");
        self.set_field("shas", json);
    }

    /// Remove the entire "nota-bene" key from the cell metadata.
    pub fn clear(&mut self) {
        self.additional.remove("nota-bene");
    }
}

// ---------------------------------------------------------------------------
// Read nota-bene meta from raw additional map
// ---------------------------------------------------------------------------

pub fn read_nota_bene(additional: &HashMap<String, Value>) -> Option<NotaBeneData> {
    match additional.get("nota-bene") {
        None => None,
        Some(v) => {
            let data: NotaBeneData = serde_json::from_value(v.clone()).unwrap_or_default();
            Some(data)
        }
    }
}

// ---------------------------------------------------------------------------
// Serde helpers
// ---------------------------------------------------------------------------

mod source_lines {
    use serde::{Deserializer, Serialize as _, Serializer};

    pub fn serialize<S>(value: &String, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        value.serialize(serializer)
    }

    pub fn deserialize<'de, D>(deserializer: D) -> Result<String, D::Error>
    where
        D: Deserializer<'de>,
    {
        struct Visitor;
        impl<'de> serde::de::Visitor<'de> for Visitor {
            type Value = String;
            fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
                write!(f, "string or array of strings")
            }
            fn visit_str<E: serde::de::Error>(self, v: &str) -> Result<String, E> {
                Ok(v.to_string())
            }
            fn visit_string<E: serde::de::Error>(self, v: String) -> Result<String, E> {
                Ok(v)
            }
            fn visit_seq<A: serde::de::SeqAccess<'de>>(
                self,
                mut seq: A,
            ) -> Result<String, A::Error> {
                let mut out = String::new();
                while let Some(s) = seq.next_element::<String>()? {
                    out.push_str(&s);
                }
                Ok(out)
            }
        }
        deserializer.deserialize_any(Visitor)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::collections::HashMap;

    fn make_additional(nb_val: serde_json::Value) -> HashMap<String, serde_json::Value> {
        let mut m = HashMap::new();
        m.insert("nota-bene".to_string(), nb_val);
        m
    }

    // --- read_nota_bene ---

    #[test]
    fn read_absent_key_returns_none() {
        assert!(read_nota_bene(&HashMap::new()).is_none());
    }

    #[test]
    fn read_present_key_returns_some() {
        let add = make_additional(json!({"diff": "some diff"}));
        assert!(read_nota_bene(&add).is_some());
    }

    #[test]
    fn read_malformed_value_returns_some_default() {
        // A non-object value silently falls back to Default.
        let add = make_additional(json!(42));
        let data = read_nota_bene(&add).expect("expected Some");
        assert!(data.diff.is_none());
        assert!(data.fixtures.is_none());
        assert!(data.test.is_none());
    }

    // --- source_lines serde (tested through Fixture / TestMeta) ---

    #[test]
    fn fixture_source_from_plain_string() {
        let f: Fixture =
            serde_json::from_str(r#"{"description":"d","priority":1,"source":"x = 1"}"#).unwrap();
        assert_eq!(f.source, "x = 1");
    }

    #[test]
    fn fixture_source_from_array_of_strings() {
        let f: Fixture = serde_json::from_str(
            r#"{"description":"d","priority":1,"source":["x = 1\n","y = 2"]}"#,
        )
        .unwrap();
        assert_eq!(f.source, "x = 1\ny = 2");
    }

    #[test]
    fn fixture_source_from_empty_array() {
        let f: Fixture =
            serde_json::from_str(r#"{"description":"d","priority":1,"source":[]}"#).unwrap();
        assert_eq!(f.source, "");
    }

    #[test]
    fn test_meta_source_from_array() {
        let t: TestMeta =
            serde_json::from_str(r#"{"name":"my_test","source":["assert True\n","assert 1==1"]}"#)
                .unwrap();
        assert_eq!(t.source, "assert True\nassert 1==1");
    }

    // --- Option<T> on NotaBeneData ---

    #[test]
    fn nota_bene_data_diff_absent_is_none() {
        let data: NotaBeneData = serde_json::from_str(r#"{}"#).unwrap();
        assert!(data.diff.is_none());
    }

    #[test]
    fn nota_bene_data_diff_null_is_none() {
        // null and absent both deserialize to None
        let data: NotaBeneData = serde_json::from_str(r#"{"diff":null}"#).unwrap();
        assert!(data.diff.is_none());
    }

    #[test]
    fn nota_bene_data_diff_value_is_some() {
        let data: NotaBeneData = serde_json::from_str(r#"{"diff":"some patch"}"#).unwrap();
        assert!(matches!(data.diff, Some(_)));
    }

    #[test]
    fn nota_bene_data_fixtures_null_is_none() {
        let data: NotaBeneData = serde_json::from_str(r#"{"fixtures":null}"#).unwrap();
        assert!(data.fixtures.is_none());
    }

    #[test]
    fn nota_bene_data_extra_fields_preserved() {
        // Unknown keys land in `extra` and survive round-trips.
        let add = make_additional(json!({"editor": {"role": "test"}, "diff": null}));
        let data = read_nota_bene(&add).expect("expected Some");
        assert!(data.extra.contains_key("editor"));
    }

    // --- NotaBeneView ---

    fn fresh() -> HashMap<String, serde_json::Value> {
        HashMap::new()
    }

    #[test]
    fn view_ensure_nb_object_creates_empty_object() {
        let mut add = fresh();
        NotaBeneView::new(&mut add).ensure_nb_object();
        assert!(add["nota-bene"].is_object());
    }

    #[test]
    fn view_ensure_nb_object_is_idempotent() {
        let mut add = fresh();
        {
            let mut v = NotaBeneView::new(&mut add);
            v.ensure_nb_object().insert("x".to_string(), json!(1));
        }
        // Calling again should not overwrite existing content.
        NotaBeneView::new(&mut add).ensure_nb_object();
        assert_eq!(add["nota-bene"]["x"], json!(1));
    }

    #[test]
    fn view_mark_addressed_creates_nb_key() {
        let mut add = fresh();
        NotaBeneView::new(&mut add).mark_addressed();
        assert!(add.contains_key("nota-bene"));
    }

    #[test]
    fn view_set_diff_stores_string() {
        let mut add = fresh();
        NotaBeneView::new(&mut add).set_diff(Some("patch".to_string()));
        assert_eq!(add["nota-bene"]["diff"], json!("patch"));
    }

    #[test]
    fn view_set_diff_none_removes_key() {
        let mut add = fresh();
        NotaBeneView::new(&mut add).set_diff(Some("x".to_string()));
        NotaBeneView::new(&mut add).set_diff(None);
        assert!(!add["nota-bene"].as_object().unwrap().contains_key("diff"));
    }

    #[test]
    fn view_set_fixtures_some_stores_map() {
        let mut add = fresh();
        let mut map = IndexMap::new();
        map.insert(
            "f1".to_string(),
            Fixture {
                description: "desc".to_string(),
                priority: 1,
                source: "x = 1".to_string(),
            },
        );
        NotaBeneView::new(&mut add).set_fixtures(Some(map));
        assert!(add["nota-bene"]["fixtures"].is_object());
        assert_eq!(add["nota-bene"]["fixtures"]["f1"]["priority"], json!(1));
    }

    #[test]
    fn view_set_fixtures_none_removes_key() {
        let mut add = fresh();
        // First write a fixture, then set to None → key removed
        let mut map = IndexMap::new();
        map.insert(
            "f1".to_string(),
            Fixture {
                description: "d".to_string(),
                priority: 0,
                source: "x = 1".to_string(),
            },
        );
        NotaBeneView::new(&mut add).set_fixtures(Some(map));
        NotaBeneView::new(&mut add).set_fixtures(None);
        assert!(!add["nota-bene"]
            .as_object()
            .unwrap()
            .contains_key("fixtures"));
    }

    #[test]
    fn view_set_test_stores_name_and_source() {
        let mut add = fresh();
        NotaBeneView::new(&mut add).set_test(Some(TestMeta {
            name: "my_test".to_string(),
            source: "assert True".to_string(),
        }));
        assert_eq!(add["nota-bene"]["test"]["name"], json!("my_test"));
        assert_eq!(add["nota-bene"]["test"]["source"], json!("assert True"));
    }

    #[test]
    fn view_set_test_none_removes_key() {
        let mut add = fresh();
        NotaBeneView::new(&mut add).set_test(Some(TestMeta {
            name: "t".to_string(),
            source: "s".to_string(),
        }));
        NotaBeneView::new(&mut add).set_test(None);
        assert!(!add["nota-bene"].as_object().unwrap().contains_key("test"));
    }

    #[test]
    fn view_set_shas_stores_entries() {
        let mut add = fresh();
        let shas = vec![ShaEntry {
            cell_id: "c1".to_string(),
            sha: "abc123".to_string(),
        }];
        NotaBeneView::new(&mut add).set_shas(shas);
        assert_eq!(add["nota-bene"]["shas"][0]["cell_id"], json!("c1"));
        assert_eq!(add["nota-bene"]["shas"][0]["sha"], json!("abc123"));
    }

    #[test]
    fn view_remove_field_removes_existing_key() {
        let mut add = fresh();
        NotaBeneView::new(&mut add).set_diff(Some("d".to_string()));
        NotaBeneView::new(&mut add).remove_field("diff");
        assert!(!add["nota-bene"].as_object().unwrap().contains_key("diff"));
    }

    #[test]
    fn view_remove_field_is_no_op_when_key_absent() {
        let mut add = fresh();
        NotaBeneView::new(&mut add).mark_addressed();
        // Should not panic.
        NotaBeneView::new(&mut add).remove_field("nonexistent");
    }

    #[test]
    fn view_remove_field_is_no_op_when_nb_key_missing() {
        let mut add = fresh();
        // nota-bene key doesn't exist; should not panic.
        NotaBeneView::new(&mut add).remove_field("diff");
    }
}
