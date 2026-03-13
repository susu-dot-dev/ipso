---
name: nota-bene in-kernel API
overview: Implement the nota_bene in-kernel Python library — the public API (execute_cell, subtest, register_teardown), the runner-facing API (_runner submodule), and shared mutable state (_state module).
todos: []
isProject: false
---

# nota-bene in-kernel API

## Context

The `nota_bene` package runs inside a Jupyter kernel. It has two consumers:

- **Test authors** (and fixture source code): use the public API — `execute_cell()`, `subtest()`, and `register_teardown()`.
- **The pytest runner** (the `pytest-nota-bene` package, out of scope here): communicates with the kernel over ZMQ by sending `execute_request` messages. It calls `nota_bene._runner` functions to inject data into the kernel and reads results back out via `nota_bene._runner.get_test_results()`.

Data flows **both ways**:

- **Into the kernel**: the runner calls `nota_bene._runner.load_cell(source)` to inject the patched cell source before the test source runs.
- **Out of the kernel**: the runner calls `nota_bene._runner.get_test_results()` after the test source finishes to retrieve the accumulated subtest results as a JSON string.

The runner applies the unified diff (if any) to the cell source **before** calling `load_cell()`. No diff logic is needed inside the kernel.

For cells in the **cumulative chain** (all cells preceding the target), the runner executes their source directly via `execute_request` — it does not use `load_cell()` or `execute_cell()` for those. `load_cell()` is called exactly once per test, for the target cell only.

This package is **unit-test only**: tests assert on Python state (variables, dataframes, return values) that the cell leaves in the kernel namespace. stdout, stderr, and rich outputs (plots, display objects) are out of scope.

## Module structure

```
nota-bene/src/nota_bene/
  __init__.py      # exposes public API
  __about__.py     # __version__
  _state.py        # shared mutable state
  _runner.py       # runner-facing functions
```

`__init__.py` and `_runner.py` both import from `_state.py`. Neither imports the other, avoiding circular imports.

---

## 1. `nota_bene._state`

Internal module. Holds all mutable state shared between the public API and `_runner`. No functions — just module-level variables initialized to their empty/default values.

### Variables

| Variable | Type | Initial value | Description |
|---|---|---|---|
| `cell_source` | `str \| None` | `None` | The patched cell source string. Set by `_runner.load_cell()`, read by `execute_cell()`. |
| `test_results` | `list[dict]` | `[]` | Accumulated subtest result dicts for the current test. Read by the runner after the test source finishes. |
| `teardown_stack` | `list[Callable]` | `[]` | LIFO stack of teardown callbacks. |

No reset function is provided — each test runs in a fresh kernel, so state starts from initial values automatically.

---

## 2. `nota_bene._runner`

Semi-private submodule. Called by the pytest runner via `execute_request` messages. Not intended for test authors.

### `load_cell(source: str) -> None`

Stores the patched cell source string so that `execute_cell()` knows what to run.

**Behavior:**

1. Store `source` in `_state.cell_source`.

The runner is solely responsible for producing the correct `source` value before calling this — joining the cell's `source` array and applying any unified diff. This function does no validation or transformation.

Called exactly once per test, for the target cell. For cells in the cumulative chain, the runner sends their source directly as `execute_request` messages and does not use `load_cell()`.

### `get_test_results() -> str`

Returns the accumulated subtest results as a JSON string.

**Behavior:**

1. Serialize `_state.test_results` to a JSON string via `json.dumps()`.
2. Return the string.

Called by the runner after the test source finishes executing to read the results out of the kernel. Returning a JSON string keeps the interface simple — the runner receives a plain string over ZMQ and deserializes it with `json.loads()`.

### `run_teardowns() -> None`

Drains `_state.teardown_stack` in LIFO order.

**Behavior:**

1. While `_state.teardown_stack` is non-empty, pop the last item and call it with no arguments.
2. If a callback raises an exception, record or log the error but continue draining the stack. All registered callbacks must be called regardless of individual failures.
3. After all callbacks have been called, `_state.teardown_stack` is empty.

Called by the runner after the test source finishes executing, before kernel shutdown.

---

## 3. `nota_bene` public API

Exposed via `__init__.py`. This is what test authors and fixture source code use. `test_results` is intentionally not exposed here — only the runner needs it, and it accesses it via `_runner.get_test_results()`.

### `execute_cell() -> None`

Runs the patched cell source in the kernel's global namespace.

**Behavior:**

1. Read `_state.cell_source`. If `None`, raise an error — `_runner.load_cell()` was not called before this.
2. Execute the source string in the kernel's global namespace via `exec()`.
3. If the cell raises an exception, it propagates to the caller — the test source sees it and fails unless the test code catches it explicitly.

Can be called multiple times within a single test. Each call re-executes the same source. The source does not change between calls — `_state.cell_source` is only updated by the runner.

### `subtest(name: str)` — context manager

Records a named subtest result. Used in data-driven tests to run multiple cases and track each independently.

**Behavior:**

1. **On enter**: record the subtest name.
2. **On exit without exception**: append the following dict to `_state.test_results`:
   ```python
   {"name": name, "passed": True, "error": None, "traceback": None}
   ```
3. **On exit with exception**: append the following dict to `_state.test_results`:
   ```python
   {"name": name, "passed": False, "error": str(exc), "traceback": formatted_traceback}
   ```
   Then **suppress the exception** — execution continues with the next statement after the `with` block.

`formatted_traceback` is a string containing the kernel-side traceback — the source lines and location of the failure, not runner machinery. This is what gets surfaced in pytest output.

One failing subtest does not stop others from running. Each `subtest()` block is independent.

### `register_teardown(callback: Callable) -> None`

Pushes a cleanup callback onto `_state.teardown_stack`.

**Behavior:**

1. Append `callback` to `_state.teardown_stack`.

The callback must be callable with no arguments. It will be invoked by `_runner.run_teardowns()` after the test completes, in reverse registration order.

Typically called from fixture source code, not from test code.

---

## 4. `test_results` format

The runner retrieves results via `nota_bene._runner.get_test_results()`, which returns a JSON string. Deserialized, it is a list of dicts, one per subtest:

```python
[
    {
        "name": "price=10.0 quantity=2",
        "passed": True,
        "error": None,
        "traceback": None
    },
    {
        "name": "price=0.0 quantity=5",
        "passed": False,
        "error": "AssertionError: expected 0.0, got 5.0",
        "traceback": "  File \"<test>\", line 12\n    assert (df[\"total\"] == ...).all()\nAssertionError: ..."
    }
]
```

| Key | Type | Description |
|---|---|---|
| `name` | `str` | Subtest name passed to `subtest()`, or test name for implicit subtest |
| `passed` | `bool` | Whether the subtest passed |
| `error` | `str \| None` | `str(exc)` if failed, `None` if passed |
| `traceback` | `str \| None` | Formatted kernel-side traceback if failed, `None` if passed |

If `test_results` is empty after the test source finishes (no `subtest()` calls were made), the **runner** (not this package) constructs a single implicit result using the test name. The in-kernel `nota_bene` package only accumulates results via `subtest()` — implicit subtest handling is runner-side.
