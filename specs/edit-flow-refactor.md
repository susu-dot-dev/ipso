# Refactor: SHA-based review model and metadata simplification

This spec describes changes needed to align the existing `nb edit` flow with the
new programmatic CLI design (see `specs/programmatic-cli.md`). The two core changes
are:

1. SHAs become the sole signal for "this cell has been deliberately reviewed"
2. The three-state `Option<Option<T>>` metadata format is replaced with a simple
   `Option<T>` — absent key and `null` value are equivalent and mean "no value"

## Bug fix: `nb edit --continue` does not update SHAs

`apply_editor_to_source()` in `save.rs` never writes `shas`. After `--continue`,
cells have no shas (`NotImplemented` staleness) or stale shas if source was modified.

Fix: after `apply_editor_to_source()` in `run_edit_continue()`, call a new
`accept_cells()` function to recompute and store shas for all cells that have
ipso metadata. Going through the editor IS the review; `--continue` should
stamp the shas.

---

## Change 1: Simplify `IpsoData` in `metadata.rs`

### Before

```rust
pub struct IpsoData {
    pub fixtures: Option<Option<IndexMap<String, Fixture>>>,
    pub diff: Option<Option<String>>,
    pub test: Option<Option<TestMeta>>,
    pub shas: Option<Vec<ShaEntry>>,
    pub extra: IndexMap<String, Value>,
}
```

With custom serde modules (`nullable_str`, `nullable_test`, `nullable_fixtures`)
to handle the three-state serialization.

### After

```rust
pub struct IpsoData {
    pub fixtures: Option<IndexMap<String, Fixture>>,
    pub diff: Option<String>,
    pub test: Option<TestMeta>,
    pub shas: Option<Vec<ShaEntry>>,
    pub extra: IndexMap<String, Value>,
}
```

Standard `Option<T>` serde with `#[serde(skip_serializing_if = "Option::is_none")]`.
The custom `nullable_*` serde modules are removed entirely.

On disk, `null` and absent key both deserialize to `None`. On write, `None` means
the key is omitted (not written as `null`). This is a breaking change to the
on-disk format for notebooks that have explicit `null` values — that's acceptable
since we're not targeting backwards compatibility.

### `IpsoMeta` enum removed

The `IpsoMeta` enum (`Absent` / `Present`) is removed entirely. All callers
that currently match on it switch to `Option<IpsoData>`:

```rust
// Before
pub fn ipso(&self) -> IpsoMeta { ... }

// After
pub fn ipso(&self) -> Option<IpsoData> { ... }
```

`None` means no `"ipso"` key in the cell metadata.
`Some(data)` means the key exists; fields inside may be `None`.

The edit flow currently uses `Present` vs `Absent` to decide whether to emit a
section header (cells with metadata get the full fixture/test section; cells without
get a passthrough). This logic is preserved but simplified: emit a full section if
`ipso()` returns `Some(_)`, emit a passthrough if it returns `None`. A cell
that has a ipso key with all-None fields still gets a full section — that is
correct, it means the cell has been through the editor before.

### `IpsoView` changes

Remove `clear_fixtures()`, `clear_diff()`, `clear_test()` — callers can just call
`set_fixtures(None)` etc. The distinction was only needed to differentiate between
"remove the key" and "set to explicit null," which no longer exists.

---

## Change 2: Simplify `save.rs`

### Remove three-state preservation logic

The current `apply_editor_to_source()` has complex logic to preserve the prior
three-state using `had_fixture_cells` / `had_test_cell` booleans and checking
`prev_nb` state. This is no longer needed.

Since going through the editor IS the review (and `--continue` now stamps shas),
the editor always produces a definitive state for every field:

**Fixtures:**
- Non-empty fixture content present → `Some(map)`
- No non-empty fixture content → `None`

**Test:**
- Non-empty test source present → `Some(TestMeta)`
- No non-empty test source → `None`

**Diff:**
- Patched source differs from original → `Some(diff_string)`
- No change → `None`

Remove from `Section` struct:
- `had_fixture_cells: bool`
- `had_test_cell: bool`

Remove from `SectionBuilder`:
- `had_fixture_cells`
- `had_test_cell`
- `fixture_index`

Remove from `apply_editor_to_source()`:
- All `match prev_nb.as_present()...` preservation branches
- The `prev_nb` variable
- The `mark_addressed()` call (no longer needed — shas serve this purpose)

### Handling cells with no ipso metadata

Cells that return `None` from `ipso()` and have no content written (no
fixtures, no test, no diff) should remain untouched — don't create a ipso
key. Code cells genuinely not under test should stay clean.

Only write a ipso key if at least one field (fixtures, diff, or test) has
content, or if the cell already had a ipso key (`ipso()` returned
`Some(_)`).

---

## Change 3: Simplify `edit.rs`

All `Some(Some(x))` patterns become `Some(x)`. All `Some(None)` patterns become
`None`. Specifically:

- `if let Some(Some(fixtures)) = &data.fixtures` → `if let Some(fixtures) = &data.fixtures`
- `Some(Some(diff)) => apply_diff(...)` → `Some(diff) => apply_diff(...)`
- `Some(Some(t)) => (t.name.clone(), t.source.clone())` → `Some(t) => (...)`
- `if let Some(Some(diff)) = &data.diff` → `if let Some(diff) = &data.diff`

The section header for a cell with no fixtures no longer needs to
distinguish `null` from absent — both mean "no fixtures currently" and
a stub is emitted either way.

---

## Change 4: Add `accept_cells()` to `shas.rs`

New public function:

```rust
/// Recompute and store shas for cells with ipso metadata.
/// If `cell_indices` is Some, only those indices are accepted.
/// If None, all cells with ipso metadata are accepted.
pub fn accept_cells(nb: &mut Notebook, cell_indices: Option<&[usize]>) {
    let snapshot = compute_snapshot(nb);
    for (idx, cell) in nb.cells.iter_mut().enumerate() {
        if let Some(indices) = cell_indices {
            if !indices.contains(&idx) {
                continue;
            }
        }
        if cell.ipso().is_some() {
            let shas_slice = snapshot[..=idx].to_vec();
            cell.ipso_mut().set_shas(shas_slice);
        }
    }
}
```

Add `set_shas()` to `IpsoView` in `metadata.rs`:

```rust
pub fn set_shas(&mut self, shas: Vec<ShaEntry>) {
    let json = serde_json::to_value(&shas).expect("Vec<ShaEntry> is always serializable");
    self.set_field("shas", json);
}
```

---

## Change 5: Create `src/diagnostics.rs`

Shared diagnostic types used by both the edit flow's section header display and
the future programmatic CLI (`nb status`, `nb view`).

```rust
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum DiagnosticType {
    MissingSha,
    Stale,
    DiffConflict,
    MissingField,
    InvalidValue,
    UnknownCell,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum Severity {
    Error,
    Warning,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Diagnostic {
    pub r#type: DiagnosticType,
    pub severity: Severity,
    pub message: String,
    pub field: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CellStatus {
    pub valid: bool,
    pub diagnostics: Vec<Diagnostic>,
}
```

Add `compute_cell_diagnostics(nb: &Notebook, cell_index: usize) -> CellStatus`:

- Maps `Staleness::NotImplemented` → `missing_sha` warning
- Maps `Staleness::OutOfDate(reasons)` → one `stale` error per reason
- If cell has a diff, checks whether it applies cleanly via `apply_diff()`; if not
  → `diff_conflict` error
- Returns `CellStatus { valid: diagnostics.is_empty(), diagnostics }`

The `Staleness` enum remains as an internal detail of `shas.rs`.

---

## Change 6: Update `run_edit_continue()` in `main.rs`

```rust
fn run_edit_continue(source_path: PathBuf, force: bool) -> Result<()> {
    // ... (load notebooks, conflict detection — unchanged) ...

    save::apply_editor_to_source(&mut source_nb, &editor_nb)?;

    // NEW: stamp shas on all cells that now have ipso metadata
    shas::accept_cells(&mut source_nb, None);

    save_notebook(&source_nb, &source_path)?;
    std::fs::remove_file(&editor_path)?;
    // ...
}
```

---

## Implementation order

1. Simplify `IpsoData` in `metadata.rs` — change `Option<Option<T>>` to
   `Option<T>`, remove `nullable_*` serde modules, remove `clear_*` methods,
   add `set_shas()` to `IpsoView`
2. Update `edit.rs` — flatten `Some(Some(x))` patterns
3. Simplify `save.rs` — remove three-state preservation logic, `had_fixture_cells`,
   `had_test_cell`, `prev_nb` tracking, `mark_addressed()`
4. Add `accept_cells()` to `shas.rs`
5. Update `run_edit_continue()` in `main.rs` to call `accept_cells()`
6. Create `src/diagnostics.rs`
7. Update all tests

## Test impact

**`metadata.rs` tests**: Remove three-state tests (`diff_null_is_some_none` etc).
Update to verify `None` round-trips correctly.

**`save.rs` tests**: Many tests assert `Some(None)` — these change to `None`.
Tests for "absent stays absent when no content" remain valid.
Tests for three-state preservation (e.g. "previously null stays null") are deleted.

**`edit.rs` tests**: Minor pattern updates. Test for "stub left blank → explicit
null" changes to "stub left blank → None (field absent)."

**New tests needed**:
- `accept_cells()` stores correct shas
- `nb edit --continue` produces valid shas on all edited cells
- `compute_cell_diagnostics()` covers all diagnostic types
- Diff conflict detection

## What is not changing

- `shas.rs` staleness detection logic (internals unchanged)
- Conflict detection in `save.rs` (`check_conflicts()`) — still needed for
  detecting source changes between `edit` and `--continue`
- MCP server
- Python package
