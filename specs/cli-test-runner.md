# CLI Test Runner (`ipso test`)

## Overview

The `ipso test` command runs notebook cell tests from the command line. It generates ephemeral test notebooks in memory, executes them via `nbclient` in parallel subprocesses, and reports structured JSON results.

No temp files are written. Communication between Rust and Python is entirely via stdin/stdout pipes.

## CLI Surface

```
ipso test <notebook.ipynb>
    --all                    Run all cells with tests
    --filter <expr>          Same filter syntax as view/status/accept
    --python <path>          Python binary (default: "python" from PATH)
    --timeout <seconds>      Per-notebook execution timeout (default: 60)
```

Exit codes:
- `0` — all tests pass
- `1` — one or more test failures
- `2` — execution/infrastructure error (kernel crash, timeout, missing dependency, etc.)

## Architecture

```
┌─────────────────────────────────────────────────────┐
│  Rust CLI (ipso test)                          │
│                                                     │
│  1. Load notebook, apply filters                    │
│  2. For each matching cell with a test (parallel):  │
│     a. Build execution chain (fixtures + sources)   │
│     b. Generate test notebook JSON in memory        │
│     c. Spawn: python -m ipso._executor         │
│        └─ pipe notebook JSON to stdin               │
│     d. Collect stdout (executed notebook JSON)      │
│     e. Parse cell outputs, extract results          │
│  3. Aggregate results, print JSON, set exit code    │
└─────────────────────────────────────────────────────┘
         │ stdin: notebook JSON                  ▲ stdout: executed notebook JSON
         ▼                                       │
┌─────────────────────────────────────────────────────┐
│  Python (ipso._executor)                       │
│                                                     │
│  - Read notebook from stdin                         │
│  - nbclient.NotebookClient(allow_errors=True)       │
│  - Write executed notebook to stdout                │
└─────────────────────────────────────────────────────┘
         │
         ▼
┌─────────────────────────────────────────────────────┐
│  IPython Kernel (started by nbclient)               │
│                                                     │
│  - Runs fixtures, cell sources, test code           │
│  - ipso in-kernel library handles subtests     │
│  - Results collected via get_test_results()          │
└─────────────────────────────────────────────────────┘
```

All of step 2 runs in parallel — one subprocess per cell test. Each cell gets its own kernel. There is no shared state between cell tests.

## Environment Assumptions

The user is responsible for ensuring that `python` (or the binary specified by `--python`) points to an environment with:

- `ipykernel`
- `ipso` (the in-kernel library — which depends on `nbclient`, pulling in `jupyter_client`, `jupyter_core`, `pyzmq`, etc.)
- The notebook's own dependencies (pandas, etc.)

`nbclient` is a direct dependency of the `ipso` Python package, so installing `ipso` is sufficient.

## Test Notebook Generation (Rust)

For a cell at index N in the source notebook, Rust generates a test notebook with the following cells. Each cell carries a `ipso_role` tag in its metadata so Rust can map outputs back to their origin after execution.

| Phase | Metadata `ipso_role` | Content |
|-------|---------------------------|---------|
| setup | `setup` | `import ipso` |
| **For each cell 0..N (including target):** | | |
| &nbsp;&nbsp;fixtures | `fixture` | Fixture source, sorted by priority (one cell per fixture) |
| &nbsp;&nbsp;load + execute | `cell_source` | `ipso._runner.load_cell(<patched source>)` then `ipso.execute_cell()` |
| **For the target cell N only:** | | |
| &nbsp;&nbsp;test | `test` | The test source (subtests, assertions, etc.) |
| results | `results` | `print("__NB_RESULTS__" + ipso._runner.get_test_results())` |
| teardown | `teardown` | `ipso._runner.run_teardowns()` |

Every cell — preceding and target — goes through the same fixtures → `load_cell()` → `execute_cell()` sequence. The target cell simply has additional test code that runs after its `execute_cell()`.

Additional metadata on each generated cell:

- `source_cell_id`: the cell ID in the original notebook this step relates to
- `fixture_name` (on fixture cells): which fixture is being run

### Fixture execution

Each fixture’s stored `source` is emitted as a single code cell and executed in the kernel user namespace (same as a normal notebook cell). Top-level bindings are visible to the patched cell and to later steps. See `docs/fixtures.md` for scoping details.

### Diff application

The patched source for a cell is computed by applying the cell's `ipso.diff` (unified diff) to its source. If no diff exists, the unpatched source is used. This reuses the existing `diff_utils::apply_diff` logic in Rust.

## Python Executor (`ipso._executor`)

A minimal module (~15 lines) invoked as `python -m ipso._executor`:

```python
import sys

import nbformat
from nbclient import NotebookClient

nb = nbformat.reads(sys.stdin.read(), as_version=4)
try:
    NotebookClient(nb, timeout=int(sys.argv[1]) if len(sys.argv) > 1 else 60, allow_errors=True).execute()
except Exception as e:
    print(f"__NB_EXEC_ERROR__{e}", file=sys.stderr)
nbformat.write(nb, sys.stdout)
```

After execution, Rust sanitizes kernel-originated strings in [`test_runner::extract_results`](../src/test_runner.rs) and `format_error` via `sanitize_kernel_text` (ANSI CSI/OSC and most C0 controls removed; newlines and tabs kept) so JSON stdout stays readable.

Key behaviors:

- `allow_errors=True`: nbclient continues executing after cell errors instead of stopping. Every cell runs, errors are captured in cell outputs, and Rust gets the full picture.
- Timeout is passed as a positional argument from Rust.
- Even if nbclient itself raises (kernel crash, etc.), the partially-executed notebook is still written to stdout.

### Naming rationale

- `_runner.py` already exists as the in-kernel API (load cell, collect results, run teardowns — called from within the kernel).
- `_executor.py` is the out-of-kernel process (receive notebook on stdin, execute via nbclient, return on stdout — called by the Rust CLI).

## Result Extraction (Rust)

After collecting stdout from each subprocess, Rust parses the executed notebook JSON and extracts results:

### Happy path

1. Walk cells, find the one with `ipso_role: "results"`.
2. In its `outputs` array, find the `stream` output with `name: "stdout"`.
3. Look for the `__NB_RESULTS__` prefix in the text. Parse the JSON after the prefix.
4. This gives the subtest results list directly from `_runner.get_test_results()`.

### Implicit subtest

If `get_test_results()` returns `[]` (no `subtest()` calls were made):

- If the test cell has no error output: implicit pass — Rust constructs `[{"name": <test_name>, "passed": true, ...}]`.
- If the test cell has an error output: implicit fail — Rust constructs `[{"name": <test_name>, "passed": false, "error": <ename + evalue>, "traceback": <joined traceback>}]`.

### Infrastructure failure

If no `__NB_RESULTS__` marker is found (a cell before the results cell raised and `allow_errors=True` was somehow not effective, or the kernel died):

1. Walk cells looking for `output_type: "error"` in outputs.
2. Use the `ipso_role` and `source_cell_id` metadata to identify which phase failed.
3. Construct an error result (see output format below).

### Subprocess failure

If the Python process exits with a non-zero code or produces no stdout:

- Check stderr for `__NB_EXEC_ERROR__` prefix.
- Construct an error result with `phase: "executor"`.

## Output Format

```json
[
  {
    "cell_id": "compute-total",
    "test_name": "validates price calculation",
    "status": "completed",
    "subtests": [
      {"name": "price=10.0 quantity=2", "passed": true, "error": null, "traceback": null},
      {"name": "price=0.0 quantity=5", "passed": false, "error": "AssertionError: expected 0.0, got 5.0", "traceback": "  File \"<test>\", line 12\n    ..."}
    ]
  },
  {
    "cell_id": "plot-chart",
    "test_name": "renders bar chart",
    "status": "error",
    "error": {
      "phase": "fixture",
      "source_cell_id": "load-data",
      "fixture_name": "load_small_csv",
      "detail": "FileNotFoundError: [Errno 2] No such file or directory: 'sales.csv'",
      "traceback": "..."
    }
  }
]
```

### `status` values

- `"completed"` — the test ran to completion. Check `subtests` for pass/fail.
- `"error"` — infrastructure failure prevented the test from completing. Check `error` for details.

### `error.phase` values

- `"fixture"` — a fixture cell raised
- `"cell_source"` — a cumulative chain cell source raised
- `"test"` — the test cell raised an uncaught exception (no subtests)
- `"executor"` — the Python subprocess itself failed (kernel crash, timeout, missing dependency)

## Implementation

### Relationship to `edit.rs`

The test notebook generation (`test_runner.rs`) and the editor notebook generation (`edit.rs`) share the same high-level traversal — iterate code cells, read ipso metadata, sort fixtures by priority, compute patched source. However, the cell content they produce is fundamentally different:

| Concern | `edit.rs` (editor notebook) | `test_runner.rs` (test notebook) |
|---------|----------------------------|----------------------------------|
| **Purpose** | Human editing in Jupyter | Machine execution via nbclient |
| **Setup cell** | `import ipso; ipso.register_ipso_skip()` | `import ipso` |
| **Section headers** | Markdown cells with staleness, hints, and optional `guide` labels | None |
| **Fixture cells** | Raw source with `# fixture:` / `# description:` / `# priority:` comment headers | Same fixture `source` as stored in metadata (one code cell per fixture) |
| **Stub fixtures** | Emitted for cells with no fixtures (editable placeholder) | Not emitted — nothing to run |
| **Source cells** | Patched source as editable code cell, preserving original cell ID | `ipso._runner.load_cell(<json>)\nipso.execute_cell()` |
| **Cells without metadata** | Passthrough (original source, editable) | Same `load_cell()` + `execute_cell()` |
| **Test cells** | Prefixed with `%%ipso_skip`, `# test: <name>` header | Raw test source, no prefix |
| **Results cell** | None | `print("__NB_RESULTS__" + ipso._runner.get_test_results())` |
| **Teardown cell** | None | `ipso._runner.run_teardowns()` |
| **Notebook metadata** | `source_shas` for conflict detection | None |

**Do not refactor `edit.rs`.** The traversal logic is ~30 lines and the two flows produce entirely different cells. `test_runner.rs` should duplicate the traversal (iterate cells, check metadata, sort fixtures, apply diffs) and build its own cell constructors independently. The flows will likely diverge further over time.

### Rust — new files

#### `src/test_runner.rs`

Two main functions:

**`build_test_notebook(source: &Notebook, target_idx: usize) -> Result<Notebook>`**

Generates a test notebook for a single cell. Walks cells 0..=target_idx:

```
cell 0:  import ipso

for each code cell 0..=target_idx:
    if cell has fixtures (from ipso metadata):
        for each fixture sorted by priority:
            emit cell: {fixture.source as-is}
            metadata: { ipso_role: "fixture", source_cell_id, fixture_name }

    compute patched_source:
        if cell has diff: apply_diff(cell.source, diff)
        else: cell.source

    emit cell: ipso._runner.load_cell({json_dumps(patched_source)})\nipso.execute_cell()
    metadata: { ipso_role: "cell_source", source_cell_id }

if target cell has test:
    emit cell: {test.source}
    metadata: { ipso_role: "test", source_cell_id }

emit cell: print("__NB_RESULTS__" + ipso._runner.get_test_results())
metadata: { ipso_role: "results" }

emit cell: ipso._runner.run_teardowns()
metadata: { ipso_role: "teardown" }
```

Notes:
- Cells without ipso metadata still get `load_cell` + `execute_cell` (their unpatched source runs in the cumulative chain).
- Markdown and raw cells are skipped entirely (same as `edit.rs`).
- The notebook metadata is minimal — just `kernelspec` copied from the source notebook.

**`extract_results(executed_nb: &Notebook, cell_id: &str, test_name: &str) -> CellTestResult`**

Parses the executed notebook returned by the Python subprocess:

1. Find the cell with `ipso_role: "results"`. Check its outputs for `__NB_RESULTS__` in stdout stream output. Parse the JSON suffix → subtest list.
2. If results cell has no output or has an error: walk all cells looking for `output_type: "error"` outputs. Use `ipso_role` metadata to identify the phase. Construct an error result.
3. Handle the implicit subtest case: if results JSON is `[]` and the test cell has no error, create `[{name: test_name, passed: true, ...}]`. If the test cell has an error, create a failed implicit subtest from the error output.

#### `src/main.rs` changes

Add `Command::Test` to the clap enum:

```rust
Test {
    path: PathBuf,
    #[arg(long)]
    all: bool,
    #[arg(long = "filter")]
    filters: Vec<String>,
    #[arg(long, default_value = "python")]
    python: String,
    #[arg(long, default_value_t = 60)]
    timeout: u64,
}
```

Add `run_test()`:

1. Load notebook, parse filters.
2. Collect indices of code cells that have a `ipso.test` and match the filters. Require `--all` or `--filter` (same pattern as `accept`).
3. For each matching cell, call `build_test_notebook()` to get the notebook JSON string.
4. Spawn all subprocesses in parallel:
   - Command: `{python} -m ipso._executor {timeout}`
   - Pipe notebook JSON to stdin.
   - Collect stdout and stderr.
   - Use `std::thread::spawn` or similar to run them concurrently.
5. For each completed subprocess, parse stdout as a notebook, call `extract_results()`.
6. Collect all `CellTestResult`s into a JSON array, print to stdout.
7. Exit code: 0 if all passed, 1 if any failed, 2 if any had status "error".

### Python — new files

#### `ipso/_executor/__init__.py`

Empty.

#### `ipso/_executor/__main__.py`

```python
"""Execute a notebook from stdin via nbclient, write the result to stdout.

Invoked by the ipso Rust CLI as: python -m ipso._executor [timeout]
"""
import sys

import nbformat
from nbclient import NotebookClient

nb = nbformat.reads(sys.stdin.read(), as_version=4)
timeout = int(sys.argv[1]) if len(sys.argv) > 1 else 60

try:
    NotebookClient(nb, timeout=timeout, allow_errors=True).execute()
except Exception as e:
    print(f"__NB_EXEC_ERROR__{e}", file=sys.stderr)

nbformat.write(nb, sys.stdout)
```

### Python — dependency change

Add `nbclient` to `ipso/pyproject.toml` under `[project.dependencies]`.

## Future: Remote Kernels

The current design assumes nbclient starts a local kernel. For remote kernels, two paths are available without changing the Rust side:

1. **`jupyter_client.GatewayClient`**: nbclient can be configured to use a remote kernel gateway via the `JUPYTER_GATEWAY_URL` environment variable. This works with zero code changes to `_executor.py`.

2. **Custom executor**: The `--python` flag could be generalized to a `--runner <command>` flag where the command is anything that accepts notebook JSON on stdin and returns executed notebook JSON on stdout. This decouples the Rust CLI from the execution backend entirely.

The abstraction boundary (notebook JSON in → executed notebook JSON out) naturally supports this evolution.
