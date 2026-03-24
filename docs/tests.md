# Tests

## Overview

Each cell can have one test. The test is plain Python source code that calls `ipso.execute_cell()` to run the cell and uses `assert` statements to validate behavior. Fixtures establish the environment before the test runs — the test code itself focuses purely on execution and assertion.

Before the cell source runs, the runner applies the cell's diff (if present) to patch the source — replacing hardcoded values like file paths with fixture-provided variables. The test code always executes the patched version.

Tests are **unit-style**: they assert on Python state left in the kernel namespace after the cell runs (variables, dataframes, computed values). stdout, stderr, and rich outputs (plots, display objects) are out of scope.

## User Scenario

A developer has a 3-cell pandas notebook. The AI has generated fixtures and tests for each cell.

**Simple test (cell 1):**

The test calls `execute_cell()` once and asserts on the result:

```python
ipso.execute_cell()
assert isinstance(df, pd.DataFrame), "expected df to be a DataFrame"
assert len(df) > 0, f"expected rows, got {len(df)}"
```

pytest output:

```
my_notebook.ipynb::loads csv data PASSED
```

**Data-driven test (cell 2):**

The test runs multiple cases, resetting state between calls:

```python
import copy

test_cases = [
    {"price": 10.0, "quantity": 2, "expected_total": 20.0},
    {"price": 0.0,  "quantity": 5, "expected_total": 0.0},
    {"price": 99.9, "quantity": 1, "expected_total": 99.9},
]

for case in test_cases:
    df["price"] = [case["price"]] * len(df)
    df["quantity"] = [case["quantity"]] * len(df)

    ipso.execute_cell()

    with ipso.subtest(f"price={case['price']} quantity={case['quantity']}"):
        assert (df["total"] == case["expected_total"]).all(), (
            f"expected total {case['expected_total']}, got {df['total'].iloc[0]}"
        )
```

pytest output:

```
my_notebook.ipynb::validates price calculation::price=10.0 quantity=2 PASSED
my_notebook.ipynb::validates price calculation::price=0.0 quantity=5 PASSED
my_notebook.ipynb::validates price calculation::price=99.9 quantity=1 PASSED
```

## Metadata Schema

A test is stored in a cell's `ipso` metadata under the `test` key, alongside `fixtures` and `diff`:

```json
{
  "ipso": {
    "fixtures": { ... },
    "diff": "...",
    "test": {
      "name": "validates price calculation",
      "source": [
        "ipso.execute_cell()\n",
        "assert isinstance(df, pd.DataFrame)\n"
      ]
    }
  }
}
```

### Fields

- **`name`** _(string, required)_: A descriptive name for the test. Used as the test identifier in reporting. Should be human-readable and meaningful — e.g. `"loads csv data"`, not `"test_cell_3"`.
- **`source`** _(array of strings, required)_: The test Python source. Each string is a line ending with `\n` (except optionally the last line), matching the `cell.source` convention in nbformat 4.

## Diff

Before `execute_cell()` runs the cell, the runner applies the cell's diff to patch the source. The diff replaces hardcoded values in the cell source with fixture-provided variables — for example, replacing a hardcoded file path with a variable set by a fixture.

The diff is stored as a standard unified diff string:

```json
{
  "ipso": {
    "diff": "--- a/cell\n+++ b/cell\n@@ -1,4 +1,4 @@\n import pandas as pd\n \n-df = pd.read_csv('huge_file.csv')\n+df = pd.read_csv(csv_name)\n df.head()\n"
  }
}
```

### How patch application works

A unified diff contains three types of lines:

- **Context lines** (start with ` `): lines expected to be present unchanged in the current source
- **Remove lines** (start with `-`): lines expected to be present and removed in the patched output
- **Add lines** (start with `+`): lines inserted into the patched output

To apply the patch, the runner joins the cell source array into a single string and walks through the diff hunks. For each context or remove line, it compares against the corresponding line in the current source. **If any line does not match exactly, the patch fails.**

This strict matching is intentional — if the cell source has changed since the diff was created, the mismatch is a signal that the diff is stale and needs regenerating. The staleness tracking system (see `staleness.md`) will typically flag this before execution reaches the diff application step, but failed patch application is a hard error with a clear message if it does occur.

Not all cells need a diff. If the cell source does not require modification to run under test, the `diff` key is absent from the metadata.

## Test Code Conventions

### Simple pattern

Call `execute_cell()` once, assert on results. Use descriptive assert messages so failures are self-explanatory:

```python
ipso.execute_cell()
assert "total" in df.columns, "expected 'total' column to be added"
assert (df["total"] == df["price"] * df["quantity"]).all(), "total column values are incorrect"
```

### Data-driven pattern

Loop over test cases, reset state between calls, wrap each case in `ipso.subtest()`:

```python
import copy
df_snapshot = copy.deepcopy(df)

for case in test_cases:
    df = copy.deepcopy(df_snapshot)  # restore state before each run
    df["price"] = [case["price"]] * len(df)

    ipso.execute_cell()

    with ipso.subtest(f"price={case['price']}"):
        assert (df["total"] == case["expected"]).all(), (
            f"expected {case['expected']}, got {df['total'].iloc[0]}"
        )
```

`ipso.subtest()` is a context manager. If an assertion inside it raises, the exception is caught and recorded as a failure for that subtest — execution continues with the next case. One failing subtest does not stop the others from running.

### Implicit subtest

If the test code never calls `ipso.subtest()`, the runner wraps the entire test in a single implicit subtest using the test `name`. The result structure is identical either way — a list with one entry instead of many.

## `ipso` API

### Properties _(read-only state)_

- **`ipso.test_results`** is not part of the public API. Test authors do not need to read it — results are accumulated internally by `subtest()` and retrieved by the runner via `ipso._runner.get_test_results()`.

### Methods _(actions)_

- **`ipso.execute_cell()`**: Runs the patched cell source in the kernel's global namespace. Raises if the cell raises. Can be called multiple times within a test — each call re-executes the cell.
- **`ipso.register_teardown(callback)`**: Registers a cleanup callback onto the teardown stack. Called from fixture source, not typically from test code.
- **`ipso.subtest(name)`**: Context manager. Records a named subtest result. Catches exceptions and marks the subtest as failed without halting the rest of the test.

### Runner-facing _(called by the runner, not test code)_

- **`ipso._runner.load_cell(source)`**: Injects the already-patched cell source string into the kernel so `execute_cell()` knows what to run.
- **`ipso._runner.get_test_results()`**: Returns accumulated subtest results as a JSON string. Called by the runner after the test source finishes.
- **`ipso._runner.run_teardowns()`**: Drains the teardown stack in LIFO order. Called by the runner after the test completes.

## Results Format

After the test source finishes executing, the runner calls `ipso._runner.get_test_results()` which returns a JSON string. Deserialized, it is a list of dicts, one per subtest:

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

The traceback is the kernel-side traceback — the actual source line that failed inside the test code, not the runner-side machinery. This is what gets surfaced in pytest output and LSP diagnostics.

## Formal Specification

### `ipso.test` schema

```json
{
  "ipso": {
    "test": {
      "name": "<string>",
      "source": ["<line1>\n", "<line2>\n"]
    }
  }
}
```

### `ipso.diff` schema

```json
{
  "ipso": {
    "diff": "<unified diff string>"
  }
}
```

### `name`

Required. Non-empty string. No character constraints — any valid JSON string is accepted. Used as the test identifier in reporting.

### `source`

Required. Array of strings. Each string is a line of Python source ending with `\n`, except optionally the last line. Joined with `""` (no separator) before execution. Same convention as fixture `source`.

### `execute_cell()` behavior

1. Read the already-patched source string previously loaded by `_runner.load_cell()`. If none was loaded, raise an error.
2. Execute the source in the kernel's global namespace via `exec()`.
3. If the cell raises an exception, it propagates to the caller.

### `subtest(name)` behavior

1. On enter: record the subtest name, note start
2. On exit without exception: append `{"name": name, "passed": True, "error": None, "traceback": None}` to `test_results`
3. On exit with exception: append `{"name": name, "passed": False, "error": str(exc), "traceback": formatted_traceback}` to `test_results`. Suppress the exception.

### Implicit subtest

If `test_results` is empty after the test source finishes executing (no `subtest()` calls were made), the runner creates a single implicit result:

- If the test source raised an uncaught exception: `{"name": test.name, "passed": False, "error": ..., "traceback": ...}`
- If the test source completed without exception: `{"name": test.name, "passed": True, "error": None, "traceback": None}`

### `test_results` format

List of dicts with the following keys:

| Key | Type | Description |
|---|---|---|
| `name` | `str` | Subtest name, or test name for implicit subtest |
| `passed` | `bool` | Whether the subtest passed |
| `error` | `str \| None` | Error message if failed, `None` if passed |
| `traceback` | `str \| None` | Formatted kernel-side traceback if failed, `None` if passed |
