---
name: Test editor CLI command (edit)
overview: Add `edit` and `edit --continue` CLI subcommands that open a notebook in test-editor mode — converting it to an editor notebook for asynchronous editing, then folding changes back into the source notebook when `--continue` is called.
todos: []
isProject: false
---

# Test Editor CLI Command

## Motivation

Fixtures, patches, and tests live in cell metadata — invisible in a normal notebook UI. The test-editor mode explodes this metadata into visible, editable cells so a developer can iterate interactively: tweak fixtures, adjust the patched source, write tests, and verify everything at once. The workflow is split into two non-blocking commands: `edit` creates the editor notebook and exits immediately, and `edit --continue` folds the changes back into the source notebook when the developer is done editing.

---

## Commands

### `ipso edit <path>`

Creates the editor notebook and exits. Non-blocking.

1. Creates `<stem>.ipso.ipynb` in the same directory as the source (e.g. `analysis.ipynb` → `analysis.ipso.ipynb`). Exits with an error if the editor file already exists.
2. Computes a SHA snapshot of every cell in the source notebook and stores it in the editor notebook's metadata (see [Notebook-level metadata](#notebook-level)).
3. Prints the path to the editor notebook and exits.

- **`<path>`**: Path to the source `.ipynb` file.
- **Error if editor file exists**: If `<stem>.ipso.ipynb` already exists, exit non-zero with a message like:
  ```
  Editor notebook already exists: analysis.ipso.ipynb
  Use `ipso edit --continue analysis.ipynb` to apply your changes, or
      `ipso edit --clean analysis.ipynb` to discard it and start fresh.
  ```

### `ipso edit --continue <path>`

Applies the editor notebook changes back to the source notebook.

1. Reads the editor notebook from `<stem>.ipso.ipynb`. Exits with an error if it does not exist.
2. Performs conflict detection by comparing the stored `source_shas` against the current state of the source notebook (see [Conflict Detection](#step-2-conflict-detection)). If any cell has changed, aborts and reports which cells differ.
3. Applies the changes back to the source notebook in place.
4. Deletes the editor notebook.
5. Prints a confirmation message.

- **`<path>`**: Path to the source `.ipynb` file.
- **`--force`**: Skip conflict detection. Before merging, strips all `ipso` metadata from every cell in the source notebook (i.e. removes the `ipso` key from `cell.metadata.additional` entirely for every cell), then applies the editor notebook's changes unconditionally. Use this to discard source-side changes and start fresh from the editor state.

### `ipso edit --clean <path>`

Discards any in-progress editor notebook and creates a fresh one from the current source.

1. If `<stem>.ipso.ipynb` exists, deletes it.
2. Creates a new `<stem>.ipso.ipynb` from the current source notebook (same as `edit`).
3. Prints a confirmation message.

- **`<path>`**: Path to the source `.ipynb` file.

---

## Metadata Convention: `ipso.editor`

All editor-specific data — both notebook-level provenance and per-cell role information — lives under the `editor` subkey within the existing `ipso` object. This keeps the namespace unified and makes editor state easy to identify and strip: the apply step removes the `editor` subkey from every cell's metadata and from the notebook-level metadata before writing back to the source notebook. Nothing editor-specific leaks into normal notebook mode.

### Notebook-level

Stored in `nb.metadata.additional["ipso"]["editor"]`:

```json
{
  "metadata": {
    "ipso": {
      "editor": {
        "source_path": "analysis.ipynb",
        "source_shas": [
          {"cell_id": "abc123", "sha": "a1b2c3d4e5f6..."},
          {"cell_id": "def456", "sha": "d4e5f6a1b2c3..."}
        ]
      }
    }
  }
}
```

- **`source_path`**: Absolute path to the original notebook.
- **`source_shas`**: SHA snapshot of every cell in the source notebook at edit time, using the same algorithm as `staleness.md`. Used during the apply step for conflict detection.

In Rust:

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EditorNotebookMeta {
    pub source_path: String,
    pub source_shas: Vec<ShaEntry>,  // ShaEntry from shas.rs
}
```

Read/write via `serde_json::from_value` / `serde_json::to_value` at `nb.metadata.additional["ipso"]["editor"]`.

### Cell-level

Stored in `cell.metadata.additional["ipso"]["editor"]` on every cell the `edit` command creates. The shape varies by cell role; see each cell type below. The general form is:

```json
{
  "ipso": {
    "editor": {
      "role": "<role>",
      "cell_id": "<source_cell_id>"
    }
  }
}
```

`role` is one of: `"setup"`, `"section-header"`, `"guide"`, `"fixture"`, `"patched-source"`, `"source"`, `"test"`.

`cell_id` refers to the **source notebook cell** the editor cell belongs to (not the editor cell's own Jupyter ID), and is present on all roles except `"setup"`.

On save, the `editor` subkey is stripped from all cells before writing to the source notebook. The patched-source cell also has its Jupyter cell ID reset to its original value (it was borrowed from the source cell as the section anchor; see [Patched Source Cell](#patched-source-cell-code-cell)).

---

## Edit: Notebook Layout

The `edit` command produces a notebook with this top-level structure:

1. **Setup cell** — always first; registers the `%%ipso_skip` IPython cell magic.
2. **Sections** — one per **code** cell in the source notebook, in source order.

Non-code cells (markdown, raw) from the source are **skipped** — they are not included in the editor notebook. The editor notebook only surfaces code cells, which are the ones that can carry test metadata.

### Setup Cell

A code cell emitted as the first cell of every editor notebook. Its purpose is to register the `%%ipso_skip` IPython cell magic so that test cells are skipped during run-all. The exact implementation is an internal detail of the `ipso` Python package (`ipso.register_ipso_skip()`).

Cell metadata:

```json
{"ipso": {"editor": {"role": "setup"}}}
```

See [Test Execution Guard](#test-execution-guard) for rationale.

---

### Section Structure

Each code cell in the source notebook becomes a **section** in the editor notebook.

#### Cell with ipso metadata (`IpsoMeta::Present`)

1. **Section header** (markdown cell)
2. **Guide** (markdown cell, role `"guide"`) — explains fixture cells
3. **Fixture cells** (code cells, zero or more) — one per fixture, in priority order (or one stub if none)
4. **Guide** (markdown cell, role `"guide"`) — explains the patched cell
5. **Patched source cell** (code cell) — carries the original cell's Jupyter ID
6. **Guide** (markdown cell, role `"guide"`) — explains the test cell
7. **Test cell** (code cell) — guarded with `%%ipso_skip`

#### Cell without ipso metadata (`IpsoMeta::Absent`)

1. **Section header** (markdown cell)
2. **Guide** (markdown cell, role `"guide"`) — explains the source cell
3. **Source cell** (code cell, role `"source"`) — the original source, carrying the original cell's Jupyter ID

---

### Section Header (markdown cell)

A single markdown cell per section that communicates: section title (including 1-based position and cell ID), status, staleness reasons (if any), and the original source as a read-only reference when the cell has a diff.

The exact markdown formatting is an implementation detail. The following information must be present:

- The cell's **1-based position** (counting only code cells in the editor notebook, not source-order) and its **cell ID**.
- The **status**: one of `Has tests`, `No tests`, or `Needs review`.
- When status is **Needs review**: the list of staleness **reasons** computed by `shas::staleness()`.
- When the cell has a **diff**: the **original source** reconstructed via `diff_utils::reconstruct_original()`, clearly marked as read-only.

#### Status values

| Status | Condition |
|---|---|
| **Has tests** | `IpsoMeta::Present` and `Staleness::Valid`. Also used when sub-keys are explicitly null — the cell has been reviewed and nothing is needed. |
| **No tests** | `IpsoMeta::Absent` — never reviewed. |
| **Needs review** | `IpsoMeta::Present` and `Staleness::OutOfDate` or `Staleness::NotImplemented` (shas missing). |

#### "Needs review" reasons

Computed by `shas::staleness()` — the stored `shas` snapshot is compared against current notebook state. Possible reasons:

- **"Cell source changed since fixtures were last validated"** — the cell's own SHA no longer matches its stored entry.
- **"Preceding cell `<id>` was modified"** — an earlier cell's SHA differs between stored and current.
- **"Preceding cell `<id>` was inserted (not present at last validation)"** — a cell ID exists in the current notebook but is absent from the stored snapshot.
- **"Preceding cell `<id>` was deleted (present at last validation, now missing)"** — a cell ID is in the stored snapshot but no longer in the notebook.
- **"Cell ordering changed since last validation"** — the sequence of cell IDs has changed.
- **"No staleness data (shas missing)"** — the cell has a `ipso` key but no `shas` entry.

Cell metadata:

```json
{"ipso": {"editor": {"role": "section-header", "cell_id": "<cell_id>"}}}
```

---

### Guide cells (markdown)

Short markdown cells with **no** reconstructable metadata beyond their role. They exist only to label fixture vs patched vs test (or plain source) regions in the Jupyter UI.

Cell metadata:

```json
{"ipso": {"editor": {"role": "guide"}}}
```

On `--continue`, the parser **skips** guide cells (like the setup cell). They must not produce warnings.

---

### Fixture Cells (code cells)

One code cell per fixture, sorted by `priority` ascending (stable sort, preserving indexmap insertion order for equal priorities). Each fixture cell begins with a comment header:

```python
# fixture: <fixture_name>
# description: <description>
# priority: <priority>
<fixture source>
```

Fixture names must be **globally unique** across the notebook (see `fixtures.md`).

The user can:
- **Rename** a fixture by editing the `# fixture:` line.
- **Change the description** by editing the `# description:` line.
- **Reorder** fixtures by moving cells (priorities reassigned on save unless explicit `# priority:` values are set).
- **Delete** a fixture by deleting the cell.
- **Add** a fixture by inserting a code cell between the section header and the patched-source cell.

Cell metadata:

```json
{"ipso": {"editor": {"role": "fixture", "cell_id": "<parent_cell_id>", "fixture_name": "<name>"}}}
```

`cell_id` is the ID of the **parent source cell**, not the fixture cell's own Jupyter ID.

---

### Patched Source Cell (code cell)

The cell source with the diff applied (or the original source if no diff exists). This cell's **Jupyter cell ID is set to the original source cell's ID** — the primary anchor the apply step uses to map sections back to source cells.

Cell metadata:

```json
{"ipso": {"editor": {"role": "patched-source", "cell_id": "<cell_id>"}}}
```

---

### Test Cell (code cell)

The test source, prefixed with `%%ipso_skip` to prevent accidental execution during run-all. If the cell has no test, an empty placeholder is emitted.

**With a test:**

```python
%%ipso_skip
# test: <test_name>
<test source>
```

**Empty placeholder (no test yet):**

```python
%%ipso_skip
# test: <unnamed>
```

To run a test interactively, the user deletes the `%%ipso_skip` line, runs the cell, then restores it when done (or leaves it off while iterating).

Cell metadata:

```json
{"ipso": {"editor": {"role": "test", "cell_id": "<cell_id>"}}}
```

---

## Test Execution Guard

### The problem

In the pytest runner, testing cell N runs only cell N's test — preceding cells execute fixtures and patched source but **not** their tests. Running all cells in the editor top-to-bottom would execute every test cell, causing:

- **State pollution** — a test that mutates kernel state (deletes a variable, alters a DataFrame) corrupts the environment for later sections.
- **Behavioral mismatch** — code that passes in the editor may fail under pytest (or vice versa) because the preceding execution path differs.

### The solution: `%%ipso_skip` cell magic

Each test cell is prefixed with `%%ipso_skip`. When IPython encounters a cell beginning with `%%magic_name`, it routes the **entire cell body** as a string to the registered magic function instead of executing it as Python. The `ipso_skip` magic ignores the body and prints a skip notice to make skipped cells visible during a run-all.

The cell body remains valid, formattable Python — linters and formatters see real code below the magic line. Stripping the guard on save is trivial: remove the first line if it equals `%%ipso_skip`.

---

## Apply: Reconstruction Algorithm (`--continue`)

When `edit --continue` is invoked, it reads both notebooks from disk, performs conflict detection, parses the editor notebook's sections, and writes reconstructed `ipso` metadata back onto the source notebook before saving it.

### Step 1: Load notebooks

1. Load the source notebook from `<path>` on disk.
2. Load the editor notebook from `<stem>.ipso.ipynb` on disk.
3. Extract `EditorNotebookMeta` from `nb.metadata.additional["ipso"]["editor"]` (for conflict detection).

### Step 2: Conflict detection

Compare `source_shas` from `EditorNotebookMeta` against the current state of the source notebook:

1. **Cell existence**: every `cell_id` in `source_shas` must still exist in the source notebook. If any cell was deleted, refuse and report which cells are missing.
2. **Cell ordering**: the order of `cell_id`s in the source notebook must match `source_shas` order. If cells were reordered, refuse.
3. **Content**: recompute SHA for every source cell via `compute_cell_sha()` and compare against `source_shas`. If any SHA differs, refuse and report which cells changed.

On any conflict, print a diagnostic to stderr and exit non-zero.

### Step 3: Parse sections

Walk the editor notebook cells in order. Skip cells where `ipso.editor.role` is `"setup"` or `"guide"`. Build sections by scanning for `role == "section-header"` cells — each header starts a new section extending until the next header (or end of notebook).

For each section:

- **`cell_id`**: from the section header's `ipso.editor.cell_id`.
- **`fixtures`**: cells with `role == "fixture"` in `ipso.editor`, **plus** untagged code cells between the section header and the patched-source cell (new fixtures added by the user). Parse comment headers: `# fixture:` → name, `# description:` → description, `# priority:` → priority (default 0). Source is everything after the comment header lines. Fixtures without a `# fixture:` comment get an auto-generated name: `fixture_<cell_id>_<index>`.
- **`patched_source`**: the cell with `role == "patched-source"`, identified by `ipso.editor` metadata or by matching Jupyter cell ID to the section's `cell_id`.
- **`test`**: the cell with `role == "test"`, or the first untagged code cell after the patched-source cell and before the next section header. Strip `%%ipso_skip` from the first line if present. Parse `# test:` for the name; source is everything after that line.

Untagged non-code cells (e.g. user-added markdown notes) within a section are ignored with a warning. Official guide cells use `role == "guide"` and are skipped without warning.

### Step 4: Compute diffs

For each section, compare the patched source against the current original cell source via `diff_utils::compute_diff()`:

- Sources **identical** → returns `None`.
- Sources **differ** → returns `Some(diff_string)`.

### Step 5: Write metadata

For each section, determine the three-state values to write and apply via `cell.ipso_mut()`:

**Fixtures:**
- Section has fixture cells with content → `Some(Some(IndexMap))`.
- No fixture cells, previously `Some(None)` (explicit null) → preserve `Some(None)`.
- No fixture cells, previously `None` (absent) → leave absent.

**Diff:**
- `compute_diff()` returned `Some(diff_string)` → `Some(Some(diff_string))`.
- `compute_diff()` returned `None`, cell previously addressed → `Some(None)` (no patch needed, explicitly clear).
- `compute_diff()` returned `None`, cell previously `Absent` → leave absent.

**Test:**
- Test cell has content beyond the comment line → `Some(Some(TestMeta { name, source }))`.
- Test cell is empty, previously `Some(None)` → preserve `Some(None)`.
- Test cell is empty, previously `Absent` → leave absent.

**Shas:**
- Preserve the existing `shas` from the source notebook unchanged. The apply step does not recompute staleness — that is the AI's responsibility via `keep_updated`.

**Editor subkey:**
- Strip `ipso.editor` from every cell's metadata before writing. The source notebook must not contain editor-mode state.

After computing all values, write the source notebook via `save_notebook()`.

### Step 6: Cleanup

Delete the editor notebook file after successfully writing the source notebook.

---

## Edge Cases

### Cell with no fixtures, no diff, no test (`Absent`)

Emitted as a section header + source cell with "No tests" status. If the user adds fixtures or a test in the editor, those are written as new metadata on save. If untouched, the cell remains `Absent`.

### Explicitly reviewed cell with no tests (all sub-keys `Some(None)`)

Emitted as a section header + empty fixture zone + patched source + empty test placeholder, with "Has tests" status. Indicates the cell has been reviewed and nothing is needed. On save, explicit nulls are preserved — the cell is not downgraded to `Absent`.

### Cell with fixtures but no test

Valid. Fixtures emitted normally. Test cell is an empty `%%ipso_skip` placeholder. On save, `test` is written as `Some(None)` if the cell was previously addressed, or left absent if it was `Absent`.

### Cell with a test but no fixtures

Valid. No fixture cells emitted. Patched source and test cell still present.

### User deletes a fixture cell

On save, the fixture is absent from the parsed section and not written to the `fixtures` dict.

### User adds a new fixture cell

The new cell has no `ipso.editor` metadata. Identified by position (between section header and patched-source cell), treated as a new fixture. The `# fixture:` comment provides the name; auto-generated if absent.

### User reorders fixture cells

On save, priorities are reassigned based on cell order: first → 0, second → 1, etc. If the user set explicit `# priority:` comment values, those are used as-is.

### User removes `%%ipso_skip` and saves

The `%%ipso_skip` strip is conditional: only strip if the first line is exactly `%%ipso_skip` (with or without trailing newline). If already removed, the source is taken as-is.

### Non-code cells (markdown, raw) in the source

Passed through as-is in the editor notebook. On save, they are skipped — only sections with cell IDs are processed.

---

## Implementation Notes

### Prior Art: CLI Branch

A previous attempt had a good approach to handling the cell metadata, that we should re-implement.
| Module | Port from CLI branch |
|---|---|
| `metadata.rs` | `IpsoMeta`, `IpsoData`, `IpsoView`, `Fixture`, `TestMeta`, `ShaEntry`, serde helpers |
| `notebook.rs` | `CellExt` trait, `load_notebook`, `save_notebook`, `resolve_cell`, `blank_cell_metadata` |
| `shas.rs` | `compute_cell_sha`, `compute_snapshot`, `staleness`, `Staleness` |
| `diff_utils.rs` | `compute_diff`, `apply_diff`, `check_diff_applies`, `reconstruct_original`, `has_conflict_markers` |

---

### The `nbformat` Crate

Do not use raw `serde_json::Value` for notebook I/O. The `nbformat` crate (v1.2) provides typed representations that catch malformed notebooks at parse time and handle version upgrades automatically.

**Parsing** returns a version enum that must be matched and upgraded:

```rust
let nb_versioned = nbformat::parse_notebook(&content)?;
let nb: nbformat::v4::Notebook = match nb_versioned {
    nbformat::Notebook::V4(nb) => nb,
    nbformat::Notebook::Legacy(nb) => nbformat::upgrade_legacy_notebook(nb)?,
    nbformat::Notebook::V3(nb) => nbformat::upgrade_v3_notebook(nb)?,
};
```

**Serializing** wraps back into the enum:

```rust
let json = nbformat::serialize_notebook(&nbformat::Notebook::V4(nb.clone()))?;
```

**Key types:**

- `nbformat::v4::Notebook` — top-level notebook with `cells: Vec<Cell>` and `metadata: Metadata`.
- `nbformat::v4::Cell` — enum: `Cell::Code { id, source, metadata, outputs, execution_count }`, `Cell::Markdown { id, source, metadata }`, `Cell::Raw { id, source, metadata }`.
- `nbformat::v4::CellMetadata` — strongly typed standard fields (`tags`, `collapsed`, `scrolled`, etc.) plus `additional: HashMap<String, Value>` for everything else, including `ipso`.
- `nbformat::v4::CellId` — a validated newtype over `String`. Jupyter cell IDs must be 1–64 characters, alphanumeric plus hyphens/underscores. Use `CellId::try_from("some-id")` or generate random ones; do not construct bare strings and cast them.
- `nbformat::v4::Metadata` — notebook-level metadata with `additional: HashMap<String, Value>` for the `ipso` key.

**Source format:** `source` on each cell variant is `Vec<String>` — an array of lines, each ending with `\n` except optionally the last. The `CellExt::source_str()` trait method joins them. When constructing new cells, split with `str::split_inclusive('\n')`.

---

### `metadata.rs`: Typed Metadata with `IpsoView`

The `ipso` object sits inside `cell.metadata.additional["ipso"]` as a `serde_json::Value`. Naively cloning it in and out on every read/write would be expensive and error-prone. The CLI branch solved this with two complementary types:

**`IpsoMeta`** — an owned snapshot for reading, obtained by calling `cell.ipso()`:

```rust
pub enum IpsoMeta {
    Absent,                      // "ipso" key missing entirely
    Present(Box<IpsoData>),  // key exists; fields inside may be absent/null/set
}
```

**`IpsoView`** — a mutable borrow of `cell.metadata.additional` for writing, obtained by calling `cell.ipso_mut()`. It holds a `&mut HashMap<String, Value>` and operates directly on the map with no intermediate clone:

```rust
pub struct IpsoView<'a> {
    additional: &'a mut HashMap<String, Value>,
}
```

All writes go through `IpsoView` methods:

```rust
view.set_fixtures(Some(map));   // writes Some(Some(map))
view.set_fixtures(None);        // writes Some(None) — explicit null
view.clear_fixtures();          // removes the key — back to Absent

view.set_diff(Some(s));
view.set_diff(None);            // explicit null: "no patch needed"
view.clear_diff();

view.set_test(Some(t));
view.set_test(None);
view.clear_test();              // not present in CLI branch — add alongside clear_diff

view.set_shas(snapshot);
view.mark_addressed();          // ensures "ipso" key exists without setting sub-keys
view.clear();                   // removes "ipso" key entirely
```

The view exposes an internal `ensure_nb_object()` method that lazily creates the `"ipso"` JSON object if absent. `remove_field(key)` removes a sub-key. These are used to implement the `editor` subkey operations (see below).

**`IpsoData`** holds the typed sub-fields for reading:

```rust
pub struct IpsoData {
    pub fixtures: Option<Option<IndexMap<String, Fixture>>>,
    pub diff:     Option<Option<String>>,
    pub test:     Option<Option<TestMeta>>,
    pub shas:     Option<Vec<ShaEntry>>,
    #[serde(flatten)]
    pub extra:    IndexMap<String, Value>,  // catches unknown keys, including "editor"
}
```

The `extra` field with `#[serde(flatten)]` is important: it preserves unknown sub-keys (like `editor`) when round-tripping through serde, so adding the `editor` subkey requires no schema changes to `IpsoData`.

---

### Three-State Metadata Semantics

Each sub-key (`fixtures`, `diff`, `test`) has three distinct states that carry semantic meaning and must be preserved faithfully across edit/apply:

| State | JSON in notebook | Rust in `IpsoData` | Meaning |
|---|---|---|---|
| **Absent** | key not present in `ipso` object | `None` | No decision made; field is unaddressed |
| **Null** | `"fixtures": null` | `Some(None)` | Explicitly reviewed; decided nothing is needed here |
| **Value** | `"fixtures": { ... }` | `Some(Some(v))` | Has actual content |

The top-level `ipso` key itself is also three-state via `IpsoMeta`:

- `IpsoMeta::Absent` — key missing from `additional` entirely: cell has never been reviewed.
- `IpsoMeta::Present(_)` — key exists: cell has been looked at, even if all sub-keys are null.

This distinction matters for `edit` status assignment:
- `Absent` → **"No tests"** — the cell is completely unaddressed.
- `Present` + all sub-keys `None` or `Some(None)` → **"Has tests"** — reviewed, nothing needed.
- `Present` + any `Some(Some(v))` + `Staleness::Valid` → **"Has tests"**.
- `Present` + `Staleness::OutOfDate` or `Staleness::NotImplemented` → **"Needs review"**.

And for write-back: explicit nulls (`Some(None)`) must not be silently collapsed to absent (`None`). If a section that had `Some(None)` fixtures is left empty by the user, write back `Some(None)`, not `None`.

The three-state serde is handled by private modules in `metadata.rs` (`nullable_fixtures`, `nullable_str`, `nullable_test`) that serialize `Some(None)` as JSON `null` and `None` as absent (via `skip_serializing_if = "Option::is_none"`). `source_lines` handles the `Vec<String>` ↔ joined `String` conversion for fixture and test source fields.

---

### `IpsoView` and the `editor` Subkey

The `editor` subkey lives inside the `ipso` object alongside `fixtures`, `diff`, `test`, and `shas`. It is accessed via `IpsoView`'s low-level helpers (`ensure_nb_object`, `remove_field`) and through `IpsoData.extra` for reading:

```rust
// Read: editor role from an owned snapshot
fn editor_meta(cell: &Cell) -> Option<serde_json::Value> {
    cell.ipso()
        .as_present()
        .and_then(|d| d.extra.get("editor"))
        .cloned()
}

// Write: set editor metadata on a cell
fn set_editor_meta(cell: &mut Cell, meta: serde_json::Value) {
    // ensure_nb_object() creates "ipso": {} if absent, then returns &mut Map
    let obj = cell.ipso_mut().ensure_nb_object();
    obj.insert("editor".to_string(), meta);
}

// Strip: remove editor subkey before writing back to source notebook
fn clear_editor_meta(cell: &mut Cell) {
    cell.ipso_mut().remove_field("editor");
}
```

`edit` calls `set_editor_meta` when constructing every editor cell. The apply step calls `clear_editor_meta` on every cell in the source notebook (including the patched-source cell, which shares its ID with a source cell) before writing, ensuring no editor state leaks into the source.

For the **notebook-level** `EditorNotebookMeta`, use `nb.metadata.additional` directly:

```rust
// Write on edit
nb.metadata.additional
    .entry("ipso".to_string())
    .or_insert_with(|| Value::Object(Default::default()))
    .as_object_mut()
    .unwrap()
    .insert("editor".to_string(), serde_json::to_value(&editor_meta)?);

// Read on save
let editor_meta: EditorNotebookMeta = nb.metadata.additional
    .get("ipso")
    .and_then(|v| v.get("editor"))
    .ok_or_else(|| anyhow!("not a ipso editor notebook (missing ipso.editor)"))?
    .clone()
    .try_into_deserialize()?;

// Strip on save (source notebook must not gain a ipso key at notebook level
// if it didn't have one; the editor notebook's key is irrelevant to the source)
// — no action needed; source notebook metadata is never touched by save.
```

---

### Rust Dependencies

Add to `Cargo.toml`:

```toml
nbformat       = "1.2"
diffy          = "0.4"
indexmap       = { version = "2", features = ["serde"] }
sha1           = "0.10"
canonical_json = "0.5"
serde_json     = "1"
anyhow         = "1"
```

Already present: `clap`, `serde`, `tokio`, `rmcp`.

---

### CLI Integration

Add to the `Command` enum in `src/main.rs`:

```rust
enum Command {
    /// Start the MCP server (stdio transport).
    Mcp,
    /// Open a notebook in test-editor mode.
    Edit {
        /// Path to the source .ipynb file.
        path: PathBuf,
        /// Apply editor notebook changes back to the source notebook.
        #[arg(long)]
        continue_: bool,
        /// Delete the editor notebook, discarding in-progress edits.
        #[arg(long)]
        clean: bool,
        /// Skip conflict detection; strip all ipso metadata from source before applying.
        #[arg(long)]
        force: bool,
    },
}
```

`edit` without flags dispatches to `run_edit(path)`. `edit --continue` dispatches to `run_edit_continue(path, force)`. `edit --clean` dispatches to `run_edit_clean(path)`.

---

### File Structure

```
src/
  main.rs          # Add Edit to Command enum; dispatch to run_edit which handles the full lifecycle
  mcp.rs           # Unchanged
  metadata.rs      # Port: IpsoMeta, IpsoData, IpsoView, Fixture, TestMeta, ShaEntry
  notebook.rs      # Port: CellExt, load_notebook, save_notebook, resolve_cell
  shas.rs          # Port: compute_cell_sha, compute_snapshot, staleness, Staleness
  diff_utils.rs    # Port: compute_diff, apply_diff, reconstruct_original, has_conflict_markers
  edit.rs          # NEW: edit command — source notebook → editor notebook
  save.rs          # NEW: apply step — editor notebook → source notebook (called from edit.rs lifecycle)
```

---

### Testing Strategy

Integration tests use `.ipynb` fixture files in `tests/fixtures/`:

1. **Round-trip**: `edit` a notebook with full metadata → run `edit --continue` without changes → source notebook metadata identical to before; no `editor` subkey in any cell; editor file deleted.
2. **New fixture via position inference**: `edit` → inject a bare code cell between section header and patched source in the editor notebook → `edit --continue` → verify new fixture written to metadata.
3. **Rename fixture via comment**: `edit` → change `# fixture: old_name` to `# fixture: new_name` → `edit --continue` → verify old key gone, new key present.
4. **Conflict detection**: `edit` → modify source notebook cell → `edit --continue` → verify non-zero exit and diagnostic naming the changed cell.
5. **`--force` flag**: `edit` → modify source notebook cell → `edit --continue --force` → verify all prior `ipso` metadata stripped from source, then editor changes applied successfully.
6. **Three-state round-trip**: notebook with `Absent`, `Some(None)`, and `Some(Some(v))` cells → `edit` → `edit --continue` unchanged → all three states preserved exactly.
7. **Staleness reasons**: notebook with stale cells (`shas` out of date) → `edit` → parse section header markdown → "Needs review" and correct reasons present.
8. **`%%ipso_skip` strip**: `edit` → verify test cells have `%%ipso_skip` first line → `edit --continue` → stripped source in metadata.
9. **Editor subkey stripped**: after `edit --continue`, load source notebook; assert no cell has a `ipso.editor` subkey.
10. **Editor file deleted**: after successful `edit --continue`, assert `<stem>.ipso.ipynb` no longer exists on disk.
11. **Existing editor file error**: run `edit` when `<stem>.ipso.ipynb` already exists → non-zero exit with message suggesting `--continue` or `--clean`.
12. **Missing editor file error**: run `edit --continue` when `<stem>.ipso.ipynb` does not exist → non-zero exit with clear error message.
13. **`--clean` deletes editor file**: run `edit --clean` → `<stem>.ipso.ipynb` deleted, source notebook untouched.
14. **`--clean` missing file error**: run `edit --clean` when no editor file exists → non-zero exit with clear error message.
