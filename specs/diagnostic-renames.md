---
name: Diagnostic renames and bug fix
overview: Rename diagnostic types and staleness enum, fix code cells with no ipso metadata being silently ignored.
todos: []
isProject: false
---

# Diagnostic Renames and Bug Fix

## Purpose

The current diagnostic type names (`Stale`, `MissingSha`) and the `Staleness` enum are implementation-centric and unclear to users and AI agents. This spec renames them to convey intent, splits the overly-broad `Stale` into two distinct cases, and fixes a bug where code cells with no `ipso` metadata are silently treated as valid.

---

## Rename Map

### `Staleness` enum (`src/shas.rs`)

| Old | New | Meaning |
|---|---|---|
| `Staleness` | `CellState` | The enum itself |
| `Staleness::Valid` | `CellState::Valid` | SHAs match, everything checks out |
| `Staleness::NotImplemented` | `CellState::Missing` | Code cell has no ipso setup at all, or has ipso but no shas |
| `Staleness::OutOfDate(Vec<String>)` | Split — see below | |

`OutOfDate` currently lumps all reasons into one variant. Split it into two variants:

| New variant | Fires when |
|---|---|
| `CellState::NeedsReview(Vec<String>)` | The target cell's own SHA changed (line 130-133 of current `shas.rs`) |
| `CellState::AncestorModified(Vec<String>)` | Any preceding-cell reason: deleted, inserted, reordered, or modified (lines 92-149) |

A single call to the renamed function can return **both** `NeedsReview` and `AncestorModified` reasons simultaneously (the current cell changed AND a predecessor changed). To handle this, change the return type:

```rust
pub struct CellStateResult {
    pub needs_review: Vec<String>,       // reasons the cell's own content is suspect
    pub ancestor_modified: Vec<String>,  // reasons from preceding cells
}

pub enum CellState {
    Valid,
    Missing,
    Changed(CellStateResult),
}
```

The `Changed` variant carries both buckets. Callers map each non-empty bucket to the corresponding diagnostic type.

### `staleness()` function → `cell_state()`

Rename the function from `staleness` to `cell_state`. Same signature otherwise.

### `DiagnosticType` enum (`src/diagnostics.rs`)

| Old | New |
|---|---|
| `MissingSha` | `Missing` |
| `Stale` | Split into `NeedsReview` and `AncestorModified` |
| `DiffConflict` | `DiffConflict` (unchanged) |
| `MissingField` | Remove (unused) |
| `InvalidValue` | Replace with `InvalidField` |
| `UnknownCell` | Remove (unused) |

Final enum:

```rust
pub enum DiagnosticType {
    Missing,
    NeedsReview,
    AncestorModified,
    DiffConflict,
    InvalidField,
}
```

### Severity changes

| Type | Old severity | New severity | Rationale |
|---|---|---|---|
| `Missing` | Warning | **Error** | AI must configure the cell — this is not optional |
| `NeedsReview` | Error | **Warning** | Cell changed but might still be fine — needs human/AI judgment |
| `AncestorModified` | Error | **Warning** | Predecessor changed but this cell might be unaffected |
| `DiffConflict` | Error | **Error** (unchanged) | Diff is definitively broken |
| `InvalidField` | — | **Error** | Metadata is structurally wrong |

---

## Bug Fix: Code cells with no `ipso` metadata

### Current behavior (buggy)

In `staleness()` (line 70-72 of `shas.rs`):

```rust
None => return Staleness::Valid,
```

Code cells with no `ipso` key are treated as valid. This means the AI is never told "hey, this code cell has no tests or fixtures — you should set it up."

### Fixed behavior

In the renamed `cell_state()`:

```rust
let data: IpsoData = match cell.ipso() {
    None => {
        if matches!(cell, Cell::Code { .. }) {
            return CellState::Missing;
        }
        return CellState::Valid;
    }
    Some(d) => d,
};
```

Markdown and raw cells remain `Valid` (they are not under ipso management). Code cells with no metadata produce `Missing`.

---

## Files Changed

### `src/shas.rs`

- Rename `Staleness` → `CellState` with new variants `Valid`, `Missing`, `Changed(CellStateResult)`.
- Add `CellStateResult` struct with `needs_review` and `ancestor_modified` fields.
- Rename `staleness()` → `cell_state()`.
- Apply the bug fix: code cells with no ipso metadata return `CellState::Missing`.
- Split the existing reason-collection logic: line 129-133 (target cell SHA changed) feeds `needs_review`; lines 92-149 (deleted, inserted, reordered, preceding modified) feed `ancestor_modified`.
- Update all tests: rename assertions from `Staleness::*` to `CellState::*`, update variant names, add new tests for the `Missing` bug fix on plain code cells.

### `src/diagnostics.rs`

- Rename `DiagnosticType::MissingSha` → `Missing`, `Stale` → remove (replaced by `NeedsReview` and `AncestorModified`).
- Remove `MissingField` and `UnknownCell`. Add `InvalidField`.
- Update `compute_cell_diagnostics` to handle `CellState::Changed(result)`: emit `NeedsReview` diagnostics for `result.needs_review` reasons and `AncestorModified` diagnostics for `result.ancestor_modified` reasons.
- Update severity assignments per the table above.
- Add `Display` impl for `DiagnosticType` returning snake_case strings (`"missing"`, `"needs_review"`, `"ancestor_modified"`, `"diff_conflict"`, `"invalid_field"`).
- Update all tests.

### `src/edit.rs`

- Update imports: `Staleness` → `CellState`.
- Update `make_section_header_with_meta`: match on `CellState::Valid`, `CellState::Missing`, `CellState::Changed(result)`.
- Update status strings:
  - `Valid` → `"Has tests"`
  - `Missing` → `"Needs review"` (with message: "No tests or fixtures configured for this cell.")
  - `Changed(result)` with `needs_review` non-empty → `"Needs review"` (with the reasons)
  - `Changed(result)` with only `ancestor_modified` → `"Needs review"` (with the ancestor reasons)
- Update action hint strings similarly.

### `src/main.rs`

- Update any references to `staleness` → `cell_state`, `Staleness` → `CellState`.

### `src/filter.rs`

- Update string literals in tests: `"stale"` → `"needs_review"` / `"ancestor_modified"`.
- Update the `diagnostics.type` filter examples in CLI help text.

### `src/view.rs`

- Update comments referencing "staleness".
- Update test assertions from `MissingSha` → `Missing`.

### `tests/cli.rs`

- Update integration test `edit_stale_cell_header_contains_needs_review_and_reason`: rename test, update string matching for new status/reason strings.

---

## Tests

### New unit tests (`src/shas.rs`)

| Test | What it checks |
|---|---|
| `plain_code_cell_returns_missing` | A code cell with no `ipso` metadata returns `CellState::Missing` |
| `markdown_cell_returns_valid` | A markdown cell with no metadata returns `CellState::Valid` |
| `code_cell_with_nb_no_shas_returns_missing` | Code cell with `ipso: {}` but no `shas` returns `Missing` |
| `changed_returns_both_buckets` | A cell where both its own SHA and a predecessor SHA differ returns `Changed` with both `needs_review` and `ancestor_modified` non-empty |
| `changed_only_ancestor` | Only predecessor changed → `Changed` with empty `needs_review`, non-empty `ancestor_modified` |
| `changed_only_self` | Only target cell changed → `Changed` with non-empty `needs_review`, empty `ancestor_modified` |

### Updated unit tests (`src/diagnostics.rs`)

Existing tests updated to use new type names. New test:

| Test | What it checks |
|---|---|
| `plain_code_cell_produces_missing_error` | A code cell with no metadata produces a `Missing` diagnostic with `Severity::Error` |

---

## Non-Goals

- No new features. This is purely a rename, severity adjustment, and bug fix.
- The `InvalidField` variant is added to the enum but no validation logic is implemented yet — that belongs in a future spec.
- No LSP changes — those are in the separate LSP spec.
