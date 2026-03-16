---
name: pytest-nota-bene plugin
overview: Implement the pytest plugin that discovers .ipynb files, shells out to the nota-bene CLI to run tests, and maps results back to pytest's collection and reporting system.
todos: []
isProject: false
---

# pytest-nota-bene Plugin

## Context

The `nota-bene test` CLI command already runs notebook cell tests. It builds ephemeral test notebooks in Rust, executes them via `python -m nota_bene._executor` subprocesses, extracts results, and outputs structured JSON. The pytest plugin should delegate to this existing machinery rather than reimplementing it.

The plugin lives in the `pytest-nota-bene` package (`pytest-nota-bene/src/pytest_nota_bene/plugin.py`). It is registered via the `pytest11` entry point already declared in `pyproject.toml`.

## User Experience

```
pip install pytest-nota-bene nota-bene
pytest
```

pytest discovers `.ipynb` files alongside regular test files:

```
test_utils.py::test_helper PASSED
my_notebook.ipynb::loads csv data PASSED
my_notebook.ipynb::validates price calculation::price=10.0 quantity=2 PASSED
my_notebook.ipynb::validates price calculation::price=0.0 quantity=5 FAILED
my_notebook.ipynb::validates price calculation::price=99.9 quantity=1 PASSED
```

Hierarchy: `notebook` :: `test name` :: `subtest name`. When there is only one subtest (explicit or implicit), the subtest level collapses to a single line.

## Execution Strategy: Shell Out to the CLI

There are several options for how the plugin reuses the existing test infrastructure:

### Option 1: Shell out to `nota-bene test` (chosen)

The plugin calls `nota-bene test <path> --filter "cell:<cell_id>"` as a subprocess and parses the JSON stdout.

**Pros:** Zero code duplication. Test notebook generation, diff application, fixture wrapping, result extraction — all stay in one place (Rust). Improvements to the CLI automatically apply to pytest. The CLI's `--filter` system already supports filtering by cell ID (`cell:<id>`), by index (`index:3`), or by test name presence (`test:!null`), so the mapping from pytest items to CLI invocations is direct.

**Cons:** Requires the `nota-bene` binary on PATH. Adds one process spawn per cell test. The CLI's JSON output format becomes a stability contract between the two packages.

### Option 2: Pure Python port

Reimplement `build_test_notebook` and `extract_results` in Python. Call `nota_bene._executor` directly.

**Pros:** No binary dependency beyond Python packages. **Cons:** Duplicates ~200 lines of notebook-building and result-extraction logic. Two implementations to keep in sync when the test notebook structure changes.

### Option 3: PyO3 Rust bindings

Expose the Rust functions as a Python extension module.

**Pros:** Single source of truth. **Cons:** Adds Rust toolchain as a build dependency. Platform-specific wheels. The logic is straightforward dict manipulation — not complex enough to justify the build infrastructure.

### Option 4: Shared Python module

Move notebook-building logic into `nota_bene` itself and have both the Rust CLI and plugin call it.

**Pros:** Single source of truth in Python. **Cons:** Makes the Rust CLI depend on Python for test notebook generation, which it currently avoids. Changes the architecture of an already-working system.

**Decision:** Option 1. The CLI already does the hard work. The plugin's job is collection and reporting — reading notebook metadata to discover tests, invoking the CLI for execution, and mapping results to pytest's model.

## Architecture

```
pytest process
  │
  ├─ Collection: read .ipynb JSON, yield collectors/items
  │
  └─ Execution (per cell test):
       1. Invoke: nota-bene test <notebook> --filter "cell:<cell_id>" --python <python> --timeout <timeout>
       2. Parse JSON stdout → CellTestResult
       3. Replay result as pytest pass/fail
```

Each `NotaBeneCellTest.runtest()` makes one CLI invocation for its specific cell. This is simple and correct — pytest already has its own parallelism story (`pytest-xdist`) so we don't need to optimize for parallel cell execution within the plugin.

## Collection Hierarchy

```
NotaBeneNotebook (pytest.Collector)      ← one per .ipynb file
  NotaBeneCellTest (pytest.Item)         ← one per cell with nota-bene.test
```

Subtests are **not** separate pytest items. They are reported within a single `NotaBeneCellTest` item. When a cell test has multiple subtests and some fail, the item fails and `repr_failure` lists each subtest's status.

Rationale: subtests are not known until the kernel runs. Collection must be fast and side-effect-free. Making subtests separate items would require either running the kernel at collection time (slow, side effects) or yielding placeholder items that dynamically expand (fragile, poor UX with pytest's collection model).

### `pytest_collect_file` hook

Claims `.ipynb` files and returns a `NotaBeneNotebook` collector:

```python
def pytest_collect_file(parent, file_path):
    if file_path.suffix == ".ipynb":
        return NotaBeneNotebook.from_parent(parent, path=file_path)
```

### `NotaBeneNotebook`

A `pytest.Collector` that reads the notebook JSON and yields a `NotaBeneCellTest` for each cell that has `nota-bene.test` metadata. Each item receives the cell ID and test name from the metadata — that's all it needs to invoke the CLI.

### `NotaBeneCellTest`

A `pytest.Item` that defers all kernel work to `runtest()`. On `runtest()`:

1. Invoke `nota-bene test <notebook_path> --filter "cell:<cell_id>"` via `subprocess.run`, capturing stdout and stderr.
2. Parse the JSON stdout. The CLI outputs an array of `CellTestResult` objects; there will be exactly one since we filtered to a single cell.
3. If the CLI exited with code 2 or the result has `status: "error"`, raise `NotaBeneTestError`.
4. If any subtest in the result has `passed: false`, raise `NotaBeneSubtestFailure`.
5. Otherwise the test passes.

`repr_failure` handles both exception types:
- For infrastructure errors: surfaces the phase, fixture name (if applicable), and kernel-side traceback.
- For subtest failures: lists every subtest with its pass/fail status, showing the traceback and error message for each failure.

`reportinfo` returns the path and display name in the form `notebook.ipynb::test name`.

## CLI Output Format

The plugin depends on the JSON structure that `nota-bene test` already produces:

```json
[
  {
    "cell_id": "compute-total",
    "test_name": "validates price calculation",
    "status": "completed",
    "subtests": [
      {"name": "price=10.0 quantity=2", "passed": true, "error": null, "traceback": null},
      {"name": "price=0.0 quantity=5", "passed": false, "error": "AssertionError: ...", "traceback": "..."}
    ]
  }
]
```

Or on error:

```json
[
  {
    "cell_id": "compute-total",
    "test_name": "validates price calculation",
    "status": "error",
    "error": {
      "phase": "fixture",
      "source_cell_id": "load-data",
      "fixture_name": "load_small_csv",
      "detail": "FileNotFoundError: ...",
      "traceback": "..."
    }
  }
]
```

Exit codes: 0 = all pass, 1 = test failures, 2 = infrastructure errors. The plugin checks the JSON rather than relying solely on exit codes, since the JSON is more informative.

## Configuration

Two pytest CLI options added via `pytest_addoption` under a `nota-bene` group:

- `--nb-python` — passed through to `nota-bene test --python` (default: `python`).
- `--nb-timeout` — passed through to `nota-bene test --timeout` (default: 60).
- `--nb-binary` — path to the `nota-bene` binary (default: `nb`, assumes it is on PATH).

## Exception Classes

Two exception classes signal different failure modes to `repr_failure`:

- `NotaBeneTestError` — wraps an infrastructure error dict (phase, fixture name, detail, traceback).
- `NotaBeneSubtestFailure` — wraps the full subtests list so `repr_failure` can show all results, not just the failures.

## Module Layout

All code goes in `pytest-nota-bene/src/pytest_nota_bene/plugin.py`. The implementation is small — collection hooks, one subprocess call, JSON parsing, and error formatting.

## Dependencies

`pytest-nota-bene/pyproject.toml` already declares `pytest` as a dependency. No additional Python package dependencies are needed since the plugin shells out to the `nota-bene` binary. However, `nota-bene` (the Python in-kernel library) must be installed in the kernel's environment, and the `nota-bene` (or `nb`) binary must be on PATH.

## Tests

Integration tests live in `pytest-nota-bene/tests/` and use `pytester` — pytest's built-in fixture for testing plugins. Each test creates a temporary directory with a `.ipynb` fixture file and runs pytest on it via `pytester.runpytest()`.

Tests should cover:

1. **Collection** — a notebook with one test cell is collected as `notebook.ipynb::test name`.
2. **Pass** — a passing test reports PASSED and exits 0.
3. **Assertion failure** — a failing subtest reports FAILED, exits 1, and shows the traceback.
4. **Multiple subtests** — all subtests appear in output regardless of individual pass/fail.
5. **Fixture error** — a fixture that raises is reported as an infrastructure error with the correct phase and fixture name.
6. **Multi-cell chain** — a test cell that depends on preceding cells' state works correctly.
7. **Diff** — a cell with `nota-bene.diff` applies the patch before execution.
8. **No interference** — regular `.py` test files are unaffected by the plugin.
9. **Binary not found** — graceful error when `nota-bene` is not on PATH.

The test fixture notebooks in `tests/fixtures/` (used by the Rust integration tests) should be reused where applicable rather than duplicating notebook construction inline.
