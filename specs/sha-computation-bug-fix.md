---
name: SHA computation bug fix
overview: Fix compute_cell_sha to hash source + ipso metadata (excluding shas), matching the formal spec in docs/staleness.md.
todos: []
isProject: false
---

# SHA Computation Bug Fix

## Problem

`compute_cell_sha()` in `src/shas.rs` only hashes the cell's source string. The formal spec in `docs/staleness.md` requires the SHA to cover **source + all ipso metadata except `shas`**:

```python
{
    "source": cell["source"],
    "ipso": {k: v for k, v in cell["metadata"].get("ipso", {}).items() if k != "shas"}
}
```

This means changes to fixtures, diff, or test metadata do not trigger staleness detection. A user could modify a fixture's source without the SHA changing.

## Fix

Change `compute_cell_sha` to construct a JSON object with two keys — `"source"` (the cell's source string) and `"ipso"` (the cell's ipso metadata with the `"shas"` key filtered out). If the cell has no ipso metadata, use an empty object `{}`. Serialize this object with `canonical_json::to_string` and hash as before.

The function already takes `&Cell` which implements `CellExt`, giving access to both `source_str()` and `additional()` (the raw metadata HashMap). The ipso metadata can be read directly from `additional().get("ipso")` as a `serde_json::Value`, filtering out the `"shas"` key from the object map before including it.

## Test impact

Most existing tests use the `sha_json()` helper which calls `compute_cell_sha()` dynamically. These tests compute SHAs for both the stored snapshot and the comparison, so they self-heal — the SHA values change but remain consistent. Tests that hardcode SHA strings will need updating.

New tests to add:
- SHA changes when fixture source changes (same cell source)
- SHA changes when diff field changes (same cell source)
- SHA changes when test field changes (same cell source)
- SHA does NOT change when shas field changes (circular dependency avoidance)
- Plain cell with no ipso metadata hashes with `"ipso": {}` (not absent)

## Files changed

| File | Change |
|---|---|
| `src/shas.rs` | Rewrite `compute_cell_sha` to include ipso metadata |
| `src/shas.rs` (tests) | Add new tests for metadata sensitivity |
