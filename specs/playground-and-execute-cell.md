# Playground CLI and execute_cell MCP Tool

## Overview

Two new features share a single implementation:

1. **`ipso playground`** — a CLI command that sets up the execution context
   for a target cell (running all preceding cells with their fixtures and diffs
   applied) and then either drops the user into an interactive Python REPL or
   runs non-interactively and streams the cell's stdout.

2. **`execute_cell` MCP tool** — allows an AI agent to run a specific cell and
   observe its stdout output. Implemented by spawning `ipso playground` in
   non-interactive mode and capturing its output.

`execute_test` is intentionally omitted: `ipso test` already covers that use
case for AI agents.

## Motivation

When writing or debugging notebook cell code, it is useful to be able to run a
cell in its full context (with all preceding cells' side effects and fixtures
applied) without committing to editing the notebook. The playground provides
this, both for human developers (interactive REPL) and for AI agents
(non-interactive stdout capture via the MCP tool).

## `ipso playground`

### CLI Signature

```
ipso playground <path>
    [--filter <key:expr>]...
    [--python <python>]
    [--interactive]
    [--non-interactive]
```

### Arguments

- `path` — path to the `.ipynb` notebook file.
- `--filter` — zero or more filter expressions (same syntax as `ipso test`,
  `ipso view`, etc.). Multiple flags are ANDed. If omitted, the last code cell
  is used as the target.
- `--python` — Python binary to use (default: `"python"`).
- `--interactive` — force interactive REPL even when stdin is not a TTY.
- `--non-interactive` — force non-interactive mode even when stdin is a TTY.
- `--interactive` and `--non-interactive` are mutually exclusive.

If neither flag is passed, the mode is determined by TTY detection:
interactive when stdin is a TTY, non-interactive otherwise.

### Target cell selection

The target cell is the **last code cell** that matches all supplied filters.
If no filters are supplied, the target is the last code cell in the notebook.
It is an error if no matching code cell exists.

### Execution context

All code cells from index 0 up to and including the target are included in the
context. For each cell in order:

1. Each fixture defined on the cell is executed (sorted by priority, ascending).
2. The cell's patched source is executed. The patched source is obtained by
   applying the cell's `ipso.diff` if one is present. If a diff is present but
   fails to apply cleanly, the command exits with an error — it never silently
   falls back to the raw source.

All code is executed in a single shared Python namespace, so variables defined
in earlier cells are available in later cells and in the REPL.

The `ipso` package is **not** used; cell sources are executed directly via
`exec()`. This means `ipso.execute_cell()` wrapping is absent — stdout from
`print()` calls goes directly to the terminal/stdout.

### Interactive mode

A Python launcher script is generated and executed as a subprocess with
inherited stdin/stdout/stderr. After all cells execute, `code.interact()` is
called with the accumulated namespace, dropping the user into a REPL.

The REPL banner identifies the notebook and target cell.

The subprocess exit code is forwarded as the exit code of `ipso playground`.

### Non-interactive mode

The same launcher script is generated but without the `code.interact()` call.
The cells execute, stdout flows naturally to the process stdout, and the process
exits. This makes `ipso playground --non-interactive` composable with shell
pipes and subprocess capture.

### Diff error handling

If any cell in the chain has a diff that fails to apply cleanly, `ipso
playground` exits with a non-zero status and a descriptive error message
identifying the cell and the failure. It never silently falls back to the raw
source.

### Python launcher script

The generated script uses `_ipso_` prefixed names to avoid polluting the user
namespace:

```python
import code as _ipso_code
_ipso_ns = {}
# Fixture: <cell_id> / <fixture_name>
exec(<json_encoded_source>, _ipso_ns)
# Cell: <cell_id>
exec(<json_encoded_source>, _ipso_ns)
# ... (one block per fixture/cell in order)
# Interactive only:
_ipso_code.interact(local=_ipso_ns,
    banner="ipso playground — <notebook_path> (up to cell <cell_id>)\n",
    exitmsg="")
```

Sources are embedded as JSON string literals (valid Python string literals)
using `serde_json::to_string()`, the same encoding used elsewhere in ipso.

The script is written to a temporary file, the Python subprocess is spawned,
and the temporary file is cleaned up after the subprocess exits.

## `execute_cell` MCP Tool

### Name

`execute_cell`

### Parameters

```json
{
  "notebook_path": "string (required) — path to the .ipynb file",
  "cell_id":       "string (required) — the cell to execute"
}
```

### Behaviour

1. Locate the current `ipso` binary via `std::env::current_exe()`.
2. Spawn:
   ```
   ipso playground <notebook_path> --filter cell:<cell_id> --non-interactive
   ```
   with stdout and stderr captured.
3. If the process exits non-zero, return the stderr as an error.
4. If the process exits zero, return the captured stdout as the tool result.

### Why subprocess instead of direct call

The MCP server and the CLI are the same binary. Delegating to a subprocess
ensures the MCP tool and the CLI always exercise identical code paths with no
duplication. It also keeps `mcp.rs` simple.

## Implementation

### `src/test_runner.rs` additions

```rust
pub struct PlaygroundCell {
    pub cell_id: String,
    pub role: PlaygroundCellRole,
    pub source: String,
}

pub enum PlaygroundCellRole {
    Fixture { name: String },
    CellSource,
}

pub fn build_playground_cells(
    source: &Notebook,
    target_idx: usize,
) -> Result<Vec<PlaygroundCell>>
```

`build_playground_cells` mirrors the loop in `build_test_notebook` but returns
structured data rather than a `Notebook`. It errors (rather than falling back)
on diff application failure.

### `src/main.rs` additions

- `Playground` variant added to the `Command` enum.
- `run_playground(path, raw_filters, python, interactive_flag, non_interactive_flag)`
  implements the full flow described above.

### `src/mcp.rs` additions

- `ExecuteCellParams` struct with `notebook_path` and `cell_id`.
- `execute_cell` tool handler that spawns the playground subprocess.
