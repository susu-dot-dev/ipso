---
name: LSP server
overview: A tower-lsp language server that watches .ipynb files and pushes nota-bene diagnostics on open and save, with SHA-based caching for performance.
todos: []
isProject: false
---

# LSP Server

## Purpose

Implement a Language Server Protocol server that passively monitors `.ipynb` notebooks and pushes nota-bene diagnostics to any LSP-compatible client (editor or AI agent). It reacts only to `textDocument/didOpen` and `textDocument/didSave`. It never modifies files.

This spec assumes the diagnostic-renames spec has been implemented: `CellState`, `cell_state()`, and the new `DiagnosticType` variants (`Missing`, `NeedsReview`, `AncestorModified`, `DiffConflict`, `InvalidField`) are in place.

---

## Dependencies

Add to `Cargo.toml`:

```toml
tower-lsp = "0.20"
lru       = "0.12"
```

`tokio` is already present. `tower-lsp` re-exports `lsp-types`.

---

## Entry Point

Add a `--lsp` flag to the existing `clap` CLI:

```
nota-bene --lsp
```

When passed, start the tower-lsp server over stdin/stdout and block until the connection closes (analogous to `--mcp`). This is the only change to `main.rs`.

---

## File Layout

| File | Purpose |
|---|---|
| `src/lsp.rs` | All LSP logic: backend struct, cell cache, analysis pipeline, diagnostic mapping |
| `src/main.rs` | Add `--lsp` flag; call `lsp::run_server().await` |

---

## Refactoring `compute_cell_diagnostics` for LSP Use

The existing `compute_cell_diagnostics` in `diagnostics.rs` does two things in one call:

1. Computes the cell's own diagnostics (DiffConflict)
2. Computes cross-cell state via `cell_state()` (Missing, NeedsReview, AncestorModified)

The LSP needs these separated so it can cache (1) by SHA and always recompute (2). Split `compute_cell_diagnostics` into two public functions:

### `compute_own_diagnostics(cell: &Cell) -> Vec<Diagnostic>`

Checks only things that depend on the cell's own content:

- If `cell.nota_bene().diff` exists, try `apply_diff(&cell.source_str(), diff)`. On failure, emit `DiffConflict`.
- Future: `InvalidField` checks would go here too.

This function takes a single `&Cell`, not the whole notebook. Its result is determined entirely by the cell's source and metadata — making it safe to cache by the cell's SHA.

### `compute_state_diagnostics(nb: &Notebook, cell_index: usize) -> Vec<Diagnostic>`

Calls `cell_state(nb, cell_index)` and maps the result:

- `CellState::Valid` → empty
- `CellState::Missing` → one `Missing` error
- `CellState::Changed(result)` → `NeedsReview` warnings for `result.needs_review`, `AncestorModified` warnings for `result.ancestor_modified`

This function requires the full notebook (cross-cell comparison) and must always be called fresh — never cached.

### `compute_cell_diagnostics` (preserved)

Keep the existing `compute_cell_diagnostics` as a convenience that calls both and concatenates the results. CLI code (`nb status`, `nb view`) continues to use this unchanged.

---

## Architecture

### `LspBackend`

```rust
struct LspBackend {
    client: Client,
    // Cell SHA → own diagnostics for that cell (DiffConflict, InvalidField).
    // Cross-cell state (Missing, NeedsReview, AncestorModified) is NOT
    // stored here — it depends on other cells and must always be recalculated.
    cell_cache: tokio::sync::Mutex<LruCache<String, Vec<Diagnostic>>>,
}
```

Cache capacity: 256 entries. `tokio::sync::Mutex` because it's held across `.await` points.

### `tower_lsp::LanguageServer` Implementation

| Method | Action |
|---|---|
| `initialize` | Return `ServerCapabilities` with `text_document_sync: TextDocumentSyncOptions { open_close: true, change: None, save: Some(SaveOptions { include_text: true }), .. }`. |
| `initialized` | Log `"nota-bene LSP ready"` via `client.log_message`. |
| `did_open` | Call `self.analyze(params.text_document.uri, params.text_document.text).await`. |
| `did_save` | Call `self.analyze(params.text_document.uri, params.text.unwrap_or_default()).await`. |

All other methods use default no-op implementations.

---

## Analysis Pipeline (`analyze`)

```rust
async fn analyze(&self, uri: Url, text: String)
```

### Step 1 — Parse

Parse `text` as `nbformat::v4::Notebook` via `serde_json::from_str`. On failure, publish empty diagnostics for `uri` (clears stale squiggles) and return.

### Step 2 — Compute Current SHAs in Parallel

For each cell, spawn a `tokio::task::spawn_blocking` to call `compute_cell_sha(cell)`. SHA computation is CPU-bound; `spawn_blocking` offloads to tokio's blocking thread pool.

Collect into `Vec<(usize, String, String)>` — `(cell_index, cell_id, sha)`.

> **Note**: `nbformat::v4::Cell` may not be `Send`. Clone the cell (or just the fields needed for SHA computation — `source` and `metadata`) before passing to `spawn_blocking`.

Wait for all tasks with `futures::future::join_all`.

### Step 3 — Own Diagnostics per Cell (SHA-Cached)

For each cell that is a code cell:

1. Look up `current_sha` in `cell_cache`.
2. **Cache hit** → reuse the cached `Vec<Diagnostic>`.
3. **Cache miss** → call `compute_own_diagnostics(cell)`. Insert result into cache keyed by `current_sha`.

This can also be parallelized via `spawn_blocking` since `compute_own_diagnostics` is CPU-bound (diff application), but in practice the cache will hit for most cells on incremental saves, so the benefit is marginal. Sequential is fine.

### Step 4 — State Diagnostics per Cell (Always Recomputed)

For each code cell, call `compute_state_diagnostics(&nb, cell_index)`. This is always fresh — it compares the cell's stored SHA snapshot against the current notebook. It is cheap (string comparisons) and does not benefit from caching.

### Step 5 — Map Diagnostics to LSP Positions

Build one `LineIndex::new(&text)` (single allocation, reused for all cells).

For each `(cell_index, diagnostic)`, call `json_path_range(&text, &jpath!["cells", cell_index, "source"])`. If `None`, skip the diagnostic.

Convert byte offsets → `lsp_types::Position` via `line_index.offset_to_position(offset)`.

All diagnostics point at the cell's source, regardless of which field triggered them. This produces the most useful editor experience: the squiggle lands on the visible code, not on internal metadata fields in the raw JSON.

Construct `lsp_types::Diagnostic`:

```rust
lsp_types::Diagnostic {
    range: Range { start, end },
    severity: Some(match diagnostic.severity {
        Severity::Error   => DiagnosticSeverity::ERROR,
        Severity::Warning => DiagnosticSeverity::WARNING,
    }),
    code: Some(NumberOrString::String(diagnostic.r#type.to_string())),
    source: Some("nota-bene".to_string()),
    message: diagnostic.message.clone(),
    ..Default::default()
}
```

### Step 6 — Publish

Collect all LSP diagnostics into a single `Vec<lsp_types::Diagnostic>` and call:

```rust
self.client.publish_diagnostics(uri, all_diagnostics, None).await;
```

An empty vec clears all squiggles — correct for a fully valid notebook.

---

## `run_server()`

```rust
pub async fn run_server() {
    let stdin  = tokio::io::stdin();
    let stdout = tokio::io::stdout();
    let (service, socket) = LspService::new(|client| LspBackend {
        client,
        cell_cache: tokio::sync::Mutex::new(LruCache::new(
            NonZeroUsize::new(256).unwrap()
        )),
    });
    Server::new(stdin, stdout, socket).serve(service).await;
}
```

---

## Error Handling

All errors inside `analyze` are non-fatal:

- JSON parse failure → clear diagnostics for the URI, return.
- `spawn_blocking` task panic → log a warning via `client.log_message`, skip that cell.
- `json_path_range` returning `None` → skip that individual diagnostic.

Never propagate errors as protocol-level failures — `didOpen` and `didSave` are notifications with no response channel.

---

## Tests

### Unit tests (`src/lsp.rs`)

Extract a testable free function:

```rust
fn compute_lsp_diagnostics(text: &str, cell_cache: &mut LruCache<String, Vec<Diagnostic>>)
    -> Vec<lsp_types::Diagnostic>
```

This contains the full pipeline (steps 1-5) without the tower-lsp `Client`. Tests call it directly.

| Test | What it checks |
|---|---|
| `parse_failure_returns_empty` | Malformed JSON → empty diagnostic list |
| `valid_notebook_no_diagnostics` | Fully accepted notebook → no diagnostics |
| `missing_cell_produces_error` | Code cell with no nota-bene → `Missing` error pointing at source range |
| `needs_review_produces_warning` | Cell source changed since accept → `NeedsReview` warning pointing at shas range |
| `ancestor_modified_produces_warning` | Preceding cell changed → `AncestorModified` warning pointing at shas range |
| `diff_conflict_produces_error` | Diff doesn't apply → `DiffConflict` error pointing at diff range |
| `cache_hit_reuses_own_diagnostics` | Second call with same text returns cached own diagnostics |
| `cache_miss_on_sha_change` | Changing source → different SHA → fresh own diagnostics computed |
| `state_diagnostics_always_fresh` | Cell unchanged but predecessor changed → `AncestorModified` appears despite cache hit on own diagnostics |
| `diagnostic_range_points_to_source` | Any diagnostic's byte range matches `json_path_range` for the cell's source |
| `missing_diagnostic_points_to_source_range` | `Missing` diagnostic uses the cell's source range |
| `markdown_cell_skipped` | Markdown cells produce no diagnostics |

### Unit tests (`src/diagnostics.rs`)

For the new split functions:

| Test | What it checks |
|---|---|
| `own_diagnostics_diff_conflict` | `compute_own_diagnostics` returns `DiffConflict` when diff doesn't apply |
| `own_diagnostics_clean_diff` | `compute_own_diagnostics` returns empty when diff applies cleanly |
| `own_diagnostics_no_metadata` | `compute_own_diagnostics` on a plain cell returns empty |
| `state_diagnostics_missing` | `compute_state_diagnostics` on a code cell with no metadata returns `Missing` |
| `state_diagnostics_valid` | `compute_state_diagnostics` on a valid accepted cell returns empty |
| `state_diagnostics_both_buckets` | Cell with both own and ancestor changes produces both `NeedsReview` and `AncestorModified` |

---

## Non-Goals

- No `textDocument/didChange` (incremental) handling.
- No hover, completion, code actions, or other LSP features.
- No cache persistence across server restarts.
- No debouncing — the doc notes ~3ms analysis time; debouncing is unnecessary.
