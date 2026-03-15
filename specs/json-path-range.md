---
name: JSON path range utility
overview: A function that takes a JSON string and a path, and returns the byte offset range of the value at that path.
todos: []
isProject: false
---

# JSON Path Range Utility

## Purpose

The LSP server needs to map cell diagnostics to line/column positions in the raw `.ipynb` JSON text. This requires knowing the byte offset of specific values (e.g., the `"source"` value of a particular cell) within the raw JSON string.

This utility is a standalone function with no LSP dependency.

## API

### `json_path_range(text, path) -> Option<Range<usize>>`

Takes a `&str` of JSON text and a slice of path segments. Returns the byte offset range (`start..end`) of the **value** at that path, or `None` if the path doesn't exist or the JSON is malformed.

### Path segments

An enum with two variants: `Key(String)` for object property lookup, and `Index(usize)` for array index lookup.

Provide `From<&str>` and `From<usize>` impls so segments can be constructed concisely. Since JSON object keys are always strings, there's no ambiguity — a `&str` always means object key, a `usize` always means array index.

Provide a `jpath!` macro for ergonomic path construction:

```rust
json_path_range(text, jpath!["cells", 0, "source"])
```

### `LineIndex`

A helper struct that precomputes newline byte offsets for a string and converts byte offsets to `(line, column)` pairs via binary search. Standard utility — needed by the LSP server to convert byte ranges to `lsp_types::Position`.

## Implementation

### Crate: `jsonc-parser`

Use `jsonc-parser::parse_to_ast()` to parse the JSON into an AST where every node carries byte offsets via the `Ranged` trait (`.start()`, `.end()`). Walk the AST following the path segments: for `Key`, find the matching `ObjectProp` by name; for `Index`, index into the `Array.elements` vec. Return the final node's range.

| Crate considered | Spans? | Status | Verdict |
|---|---|---|---|
| **jsonc-parser** | Byte range on every AST node | Active, powers dprint | **Chosen** — robust, documented ranges |
| `json-spanned-value` | Byte range on every Value | Last updated 2022, indexmap v1 | Stale dependency conflicts |
| `serde_json` | No intra-document offsets | N/A | Not viable |
| Manual string scanning | N/A | N/A | Fragile, breaks on escapes |

### Dependency

Add `jsonc-parser = "0.29"` to `Cargo.toml`.

## File layout

| File | Purpose |
|---|---|
| `src/json_path.rs` | `JsonPathSegment` enum, `jpath!` macro, `json_path_range()`, `LineIndex` |
| `src/main.rs` | Add `mod json_path;` |

## Tests

Test cases for `json_path_range`:
- Root-level object key returns the value's range
- Nested object key (two levels deep)
- Array index returns the correct element's range
- Array of objects with nested key lookup (the `.ipynb` pattern)
- Missing key returns `None`
- Array index out of bounds returns `None`
- Type mismatch (index on object, key on array) returns `None`
- Invalid JSON returns `None`
- Empty path returns the root value's range
- Multiline JSON (offsets span across lines correctly)

Test cases for `LineIndex`:
- Single-line string: all positions on line 0
- Multi-line string: correct line/column for positions after newlines
- Position at the start of a line
