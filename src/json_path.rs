//! JSON path range utility.
//!
//! Provides [`json_path_range`] for locating the byte offset range of a value
//! within a JSON string by path, and [`LineIndex`] for converting byte offsets
//! to `(line, column)` pairs.

use std::ops::Range;

use jsonc_parser::ast::{ObjectPropName, Value};
use jsonc_parser::common::Ranged;
use jsonc_parser::{parse_to_ast, CollectOptions, CommentCollectionStrategy, ParseOptions};

// ─── Path segment ────────────────────────────────────────────────────────────

/// A single segment in a JSON path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum JsonPathSegment {
    /// Object property lookup by key name.
    Key(String),
    /// Array element lookup by zero-based index.
    Index(usize),
}

impl From<&str> for JsonPathSegment {
    fn from(s: &str) -> Self {
        JsonPathSegment::Key(s.to_owned())
    }
}

impl From<String> for JsonPathSegment {
    fn from(s: String) -> Self {
        JsonPathSegment::Key(s)
    }
}

impl From<usize> for JsonPathSegment {
    fn from(i: usize) -> Self {
        JsonPathSegment::Index(i)
    }
}

// ─── jpath! macro ────────────────────────────────────────────────────────────

/// Ergonomic path construction.
///
/// ```rust,ignore
/// let path = jpath!["cells", 0usize, "source"];
/// ```
#[macro_export]
macro_rules! jpath {
    [$($seg:expr),* $(,)?] => {
        vec![$($crate::json_path::JsonPathSegment::from($seg)),*]
    };
}

// Re-export so callers can use `json_path::jpath!`.
pub use jpath;

// ─── json_path_range ─────────────────────────────────────────────────────────

/// Returns the byte offset range of the **value** at `path` within `text`.
///
/// Returns `None` when:
/// - `text` is not valid JSON / JSONC.
/// - Any segment of the path does not exist.
/// - A [`JsonPathSegment::Key`] is applied to an array value.
/// - A [`JsonPathSegment::Index`] is applied to an object value.
pub fn json_path_range(text: &str, path: &[JsonPathSegment]) -> Option<Range<usize>> {
    let parse_result = parse_to_ast(
        text,
        &CollectOptions {
            comments: CommentCollectionStrategy::Off,
            tokens: false,
        },
        &ParseOptions::default(),
    )
    .ok()?;

    let root = parse_result.value.as_ref()?;

    let mut current: &Value = root;

    for segment in path {
        match segment {
            JsonPathSegment::Key(key) => {
                let obj = match current {
                    Value::Object(o) => o,
                    _ => return None,
                };
                let prop = obj.properties.iter().find(|p| match &p.name {
                    ObjectPropName::String(s) => s.value.as_ref() == key.as_str(),
                    ObjectPropName::Word(w) => w.value == key.as_str(),
                })?;
                current = &prop.value;
            }
            JsonPathSegment::Index(idx) => {
                let arr = match current {
                    Value::Array(a) => a,
                    _ => return None,
                };
                let elem = arr.elements.get(*idx)?;
                current = elem;
            }
        }
    }

    let start = current.start();
    let end = current.end();
    Some(start..end)
}

// ─── LineIndex ────────────────────────────────────────────────────────────────

/// Precomputed newline offsets for fast byte-offset → `(line, col)` conversion.
///
/// Lines and columns are both **zero-based**.
pub struct LineIndex {
    /// Byte offset of the start of each line. Index 0 is always 0; index 1 is
    /// the position immediately after the first `\n`, etc.
    line_starts: Vec<usize>,
}

impl LineIndex {
    /// Build a `LineIndex` from `text`.
    pub fn new(text: &str) -> Self {
        let mut line_starts = vec![0usize];
        for (i, b) in text.bytes().enumerate() {
            if b == b'\n' {
                line_starts.push(i + 1);
            }
        }
        LineIndex { line_starts }
    }

    /// Convert a byte offset to a `(line, column)` pair (both zero-based).
    pub fn offset_to_position(&self, offset: usize) -> (usize, usize) {
        let line = match self.line_starts.binary_search(&offset) {
            Ok(i) => i,
            Err(i) => i - 1,
        };
        let col = offset - self.line_starts[line];
        (line, col)
    }

    /// Total number of lines (at least 1).
    pub fn line_count(&self) -> usize {
        self.line_starts.len()
    }
}

// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn slice(text: &str, range: Range<usize>) -> &str {
        &text[range.start..range.end]
    }

    // ── json_path_range ───────────────────────────────────────────────────────

    #[test]
    fn root_level_key() {
        let text = r#"{"name": "Alice", "age": 30}"#;
        let range = json_path_range(text, &jpath!["name"]).unwrap();
        assert_eq!(slice(text, range), r#""Alice""#);
    }

    #[test]
    fn root_level_numeric_value() {
        let text = r#"{"name": "Alice", "age": 30}"#;
        let range = json_path_range(text, &jpath!["age"]).unwrap();
        assert_eq!(slice(text, range), "30");
    }

    #[test]
    fn nested_object_key() {
        let text = r#"{"a": {"b": {"c": 42}}}"#;
        let range = json_path_range(text, &jpath!["a", "b", "c"]).unwrap();
        assert_eq!(slice(text, range), "42");
    }

    #[test]
    fn array_index() {
        let text = r#"[10, 20, 30]"#;
        let range = json_path_range(text, &jpath![1usize]).unwrap();
        assert_eq!(slice(text, range), "20");
    }

    #[test]
    fn array_of_objects_nested_key() {
        // Simulates the .ipynb pattern: cells[0].source
        let text = r#"{"cells": [{"source": "print(1)"}, {"source": "print(2)"}]}"#;
        let range = json_path_range(text, &jpath!["cells", 0usize, "source"]).unwrap();
        assert_eq!(slice(text, range), r#""print(1)""#);

        let range2 = json_path_range(text, &jpath!["cells", 1usize, "source"]).unwrap();
        assert_eq!(slice(text, range2), r#""print(2)""#);
    }

    #[test]
    fn missing_key_returns_none() {
        let text = r#"{"a": 1}"#;
        assert!(json_path_range(text, &jpath!["b"]).is_none());
    }

    #[test]
    fn array_index_out_of_bounds_returns_none() {
        let text = r#"[1, 2, 3]"#;
        assert!(json_path_range(text, &jpath![5usize]).is_none());
    }

    #[test]
    fn key_on_array_returns_none() {
        let text = r#"[1, 2, 3]"#;
        assert!(json_path_range(text, &jpath!["key"]).is_none());
    }

    #[test]
    fn index_on_object_returns_none() {
        let text = r#"{"a": 1}"#;
        assert!(json_path_range(text, &jpath![0usize]).is_none());
    }

    #[test]
    fn invalid_json_returns_none() {
        assert!(json_path_range("{not valid json", &jpath!["a"]).is_none());
    }

    #[test]
    fn empty_string_returns_none() {
        assert!(json_path_range("", &jpath!["a"]).is_none());
    }

    #[test]
    fn empty_path_returns_root_range() {
        let text = r#"{"a": 1}"#;
        let range = json_path_range(text, &[]).unwrap();
        assert_eq!(slice(text, range), text);
    }

    #[test]
    fn empty_path_on_array() {
        let text = r#"[1, 2]"#;
        let range = json_path_range(text, &[]).unwrap();
        assert_eq!(slice(text, range), text);
    }

    #[test]
    fn null_value() {
        let text = r#"{"x": null}"#;
        let range = json_path_range(text, &jpath!["x"]).unwrap();
        assert_eq!(slice(text, range), "null");
    }

    #[test]
    fn boolean_values() {
        let text = r#"{"t": true, "f": false}"#;
        assert_eq!(
            slice(text, json_path_range(text, &jpath!["t"]).unwrap()),
            "true"
        );
        assert_eq!(
            slice(text, json_path_range(text, &jpath!["f"]).unwrap()),
            "false"
        );
    }

    #[test]
    fn empty_array_index_returns_none() {
        let text = r#"[]"#;
        assert!(json_path_range(text, &jpath![0usize]).is_none());
    }

    #[test]
    fn empty_object_key_returns_none() {
        let text = r#"{}"#;
        assert!(json_path_range(text, &jpath!["a"]).is_none());
    }

    #[test]
    fn empty_string_key() {
        let text = r#"{"": "empty key value"}"#;
        let range = json_path_range(text, &jpath![""]).unwrap();
        assert_eq!(slice(text, range), r#""empty key value""#);
    }

    #[test]
    fn unicode_and_special_chars_in_key() {
        let text = "{\" emoji\u{1F600}\": 99}";
        let range = json_path_range(text, &jpath![" emoji\u{1F600}"]).unwrap();
        assert_eq!(slice(text, range), "99");
    }

    #[test]
    fn escaped_key_in_json() {
        // The JSON key "a\nb" (with literal backslash-n) decodes to "a\nb"
        // (with a real newline). We look up the decoded key.
        let text = r#"{"a\nb": "value"}"#;
        let range = json_path_range(text, &jpath!["a\nb"]).unwrap();
        assert_eq!(slice(text, range), r#""value""#);
    }

    #[test]
    fn multiline_json_offsets_span_lines() {
        let text = "{\n  \"key\": \"value\"\n}";
        let range = json_path_range(text, &jpath!["key"]).unwrap();
        assert_eq!(slice(text, range.clone()), r#""value""#);
        // Offset must be beyond the first newline (byte 1)
        assert!(range.start > 1);
    }

    #[test]
    fn deeply_nested_missing_intermediate_key() {
        let text = r#"{"a": {"b": 1}}"#;
        assert!(json_path_range(text, &jpath!["a", "c", "d"]).is_none());
    }

    #[test]
    fn path_through_null_returns_none() {
        let text = r#"{"a": null}"#;
        assert!(json_path_range(text, &jpath!["a", "b"]).is_none());
    }

    #[test]
    fn array_first_element() {
        let text = r#"["first", "second"]"#;
        let range = json_path_range(text, &jpath![0usize]).unwrap();
        assert_eq!(slice(text, range), r#""first""#);
    }

    #[test]
    fn array_last_element() {
        let text = r#"[1, 2, 3]"#;
        let range = json_path_range(text, &jpath![2usize]).unwrap();
        assert_eq!(slice(text, range), "3");
    }

    #[test]
    fn nested_arrays() {
        let text = r#"[[1, 2], [3, 4]]"#;
        let range = json_path_range(text, &jpath![1usize, 0usize]).unwrap();
        assert_eq!(slice(text, range), "3");
    }

    #[test]
    fn value_is_array() {
        let text = r#"{"arr": [1, 2, 3]}"#;
        let range = json_path_range(text, &jpath!["arr"]).unwrap();
        assert_eq!(slice(text, range), "[1, 2, 3]");
    }

    #[test]
    fn value_is_object() {
        let text = r#"{"obj": {"x": 1}}"#;
        let range = json_path_range(text, &jpath!["obj"]).unwrap();
        assert_eq!(slice(text, range), r#"{"x": 1}"#);
    }

    // ── LineIndex ─────────────────────────────────────────────────────────────

    #[test]
    fn line_index_single_line() {
        let idx = LineIndex::new("hello world");
        assert_eq!(idx.offset_to_position(0), (0, 0));
        assert_eq!(idx.offset_to_position(5), (0, 5));
        assert_eq!(idx.offset_to_position(10), (0, 10));
        assert_eq!(idx.line_count(), 1);
    }

    #[test]
    fn line_index_multi_line() {
        // "abc\ndef\nghi"
        //  0123 456 7 89 10
        let text = "abc\ndef\nghi";
        let idx = LineIndex::new(text);
        assert_eq!(idx.line_count(), 3);
        assert_eq!(idx.offset_to_position(0), (0, 0)); // 'a'
        assert_eq!(idx.offset_to_position(2), (0, 2)); // 'c'
        assert_eq!(idx.offset_to_position(3), (0, 3)); // '\n'
        assert_eq!(idx.offset_to_position(4), (1, 0)); // 'd' (start of line 1)
        assert_eq!(idx.offset_to_position(6), (1, 2)); // 'f'
        assert_eq!(idx.offset_to_position(8), (2, 0)); // 'g'
        assert_eq!(idx.offset_to_position(10), (2, 2)); // 'i'
    }

    #[test]
    fn line_index_position_at_line_start() {
        let text = "foo\nbar\nbaz";
        let idx = LineIndex::new(text);
        // Line 1 starts at offset 4
        assert_eq!(idx.offset_to_position(4), (1, 0));
        // Line 2 starts at offset 8
        assert_eq!(idx.offset_to_position(8), (2, 0));
    }

    #[test]
    fn line_index_empty_string() {
        let idx = LineIndex::new("");
        assert_eq!(idx.line_count(), 1);
        assert_eq!(idx.offset_to_position(0), (0, 0));
    }

    #[test]
    fn line_index_trailing_newline() {
        let text = "foo\n";
        let idx = LineIndex::new(text);
        assert_eq!(idx.line_count(), 2);
        assert_eq!(idx.offset_to_position(3), (0, 3)); // '\n' character
        assert_eq!(idx.offset_to_position(4), (1, 0)); // past the newline
    }

    #[test]
    fn line_index_only_newlines() {
        let text = "\n\n\n";
        let idx = LineIndex::new(text);
        assert_eq!(idx.line_count(), 4);
        assert_eq!(idx.offset_to_position(0), (0, 0));
        assert_eq!(idx.offset_to_position(1), (1, 0));
        assert_eq!(idx.offset_to_position(2), (2, 0));
        assert_eq!(idx.offset_to_position(3), (3, 0));
    }
}
