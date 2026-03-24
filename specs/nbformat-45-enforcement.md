---
name: nbformat 4.5 enforcement and upgrade command
overview: Reject notebooks older than nbformat 4.5 on all load paths with a helpful error, surface an upgrade-required diagnostic in the LSP, and add an `ipso upgrade` CLI command to upgrade notebooks in-place.
todos: []
isProject: false
---

# nbformat 4.5 Enforcement and `ipso upgrade` Command

## Problem

Notebooks in nbformat 4.4 and earlier have no `id` field on cells. When the
`nbformat` crate loads these notebooks via `upgrade_legacy_notebook`, it assigns
random UUID v4 IDs at deserialization time. These IDs are never written back to
the file, so every `ipso view` / `ipso update` / `ipso accept` run on a 4.4
notebook produces entirely different cell IDs. All cell-targeting operations are
non-functional on such notebooks.

The LSP silently does nothing for 4.4 notebooks: `serde_json::from_str::<v4::Notebook>`
fails (because `id` is required in the v4 schema) and returns `None`, so no
diagnostics are ever shown.

## Solution

1. **Enforce 4.5** on all notebook load paths — return a descriptive error for
   anything older, pointing the user to `ipso upgrade`.
2. **LSP** — detect the old-format case and publish a single notebook-level
   `upgrade_required` error diagnostic instead of silently doing nothing.
3. **`ipso upgrade`** — new CLI command that upgrades a notebook in-place to
   nbformat 4.5, assigning stable cell IDs (preferring `_cell_guid` from cell
   metadata when present, falling back to generated UUIDs).

## Changes

### `src/notebook.rs`

Remove the `Legacy` and `V3` upgrade arms from both `load_notebook_from_str`
and `load_notebook`. Replace them with an `anyhow::bail!`:

```rust
nbformat::Notebook::Legacy(_) | nbformat::Notebook::V3(_) => {
    bail!(
        "notebook \"{path_hint}\" is not nbformat 4.5.\n\
         Run `ipso upgrade {path_hint}` to add stable cell IDs."
    )
}
```

This affects every CLI command and MCP tool automatically.

### `src/lsp.rs`

Replace the silent `serde_json::from_str::<v4::Notebook>(text).ok()?` in
`compute_lsp_diagnostics` with a two-step parse:

1. Call `nbformat::parse_notebook(text)`. On `Err` → return `None` (not a valid
   notebook; keep previous diagnostics to avoid flicker, same as today).
2. On `Notebook::Legacy(_)` or `Notebook::V3(_)` → return
   `Some(vec![upgrade_diagnostic])` where `upgrade_diagnostic` is:
   - position `(0,0)–(0,0)`
   - severity `Error`
   - source `"ipso"`
   - code `StringOrNumber::String("upgrade_required".into())`
   - message `"Notebook is not nbformat 4.5. Run \`ipso upgrade <path>\` to add stable cell IDs."`
3. On `Notebook::V4(nb)` → proceed with `nb` directly (no second `serde_json` parse).

### `src/main.rs`

Add `Upgrade` variant to the `Command` enum:

```rust
/// Upgrade a notebook to nbformat 4.5 by assigning stable cell IDs.
///
/// Writes the upgraded notebook in-place. With --stdin, reads from stdin
/// and writes the result to stdout.
Upgrade {
    /// Path to the .ipynb file.
    path: PathBuf,
    /// Read from stdin; write upgraded notebook to stdout.
    #[arg(long)]
    stdin: bool,
    /// Show what would change without modifying the file.
    #[arg(long)]
    dry_run: bool,
},
```

Add `run_upgrade(path, stdin, dry_run) -> Result<()>`:

1. Load content from stdin or file.
2. `nbformat::parse_notebook(&content)` — bypasses the hardened `load_notebook`
   deliberately; this is the one place that must handle old formats.
3. Match:
   - `Notebook::V4(_)`: eprintln "already nbformat 4.5, nothing to do.", exit 0.
   - `Notebook::Legacy(nb)`: call `nbformat::upgrade_legacy_notebook(nb)?`.
   - `Notebook::V3(nb)`: call `nbformat::upgrade_v3_notebook(nb)?`.
4. **Post-process cell IDs**: for each upgraded cell, check
   `metadata.additional["_cell_guid"]` for a string value that satisfies the
   CellId constraints (1–64 chars, `[a-zA-Z0-9-_]+`). If valid and not already
   used by another cell (track a `HashSet<String>`), replace the generated UUID
   with that value via `CellId::new(...)`. Count `guid_count` vs
   `generated_count` for the summary message.
5. Set `nb.nbformat_minor = 5`.
6. Serialize with `nbformat::serialize_notebook`.
7. If `--dry-run` or `--stdin`: write JSON to stdout, print summary to stderr.
8. Otherwise: write JSON to the file path in-place, print summary to stderr.

Summary format: `"Upgraded <path>: <N> IDs from _cell_guid, <M> generated."`

### `tests/cli.rs`

New tests:

- **`upgrade_errors_on_legacy_notebook_via_view`** — run `ipso view` on the
  titanic fixture, assert non-zero exit and stderr contains "not nbformat 4.5".
- **`upgrade_command_upgrades_legacy_notebook`** — copy titanic to a temp file,
  run `ipso upgrade`, assert exit 0, assert `nbformat_minor == 5`, assert all
  cells have an `id` field, assert first code cell's `id` equals its
  `_cell_guid`.
- **`upgrade_command_is_idempotent`** — upgrade temp file a second time, assert
  exit 0 and "already nbformat 4.5" in stderr.
- **`upgrade_stdin`** — pipe titanic fixture through `ipso upgrade --stdin
  <path>`, assert stdout is valid JSON with `nbformat_minor == 5` and all cells
  have `id` fields, assert original file is unchanged.
- **`upgrade_dry_run`** — run `--dry-run` on a copy of titanic, assert the file
  on disk is unchanged but stdout contains upgraded JSON with `nbformat_minor == 5`.
- **`upgrade_already_v45_is_noop`** — run `ipso upgrade` on an existing 4.5
  fixture (`simple.ipynb`), assert exit 0 and stderr contains "already nbformat
  4.5".

## Files changed

| File | Change |
|---|---|
| `src/notebook.rs` | Remove legacy/v3 upgrade arms; bail with upgrade hint |
| `src/lsp.rs` | Detect old format; emit `upgrade_required` LSP diagnostic |
| `src/main.rs` | Add `Upgrade` command variant and `run_upgrade` implementation |
| `tests/cli.rs` | Six new integration tests |
