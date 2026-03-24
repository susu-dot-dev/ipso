# Fixtures

## Overview

Fixtures are lightweight stand-ins for the expensive or side-effectful operations a notebook cell normally performs. A cell that loads a 3GB CSV becomes, under test, a cell that loads a 10-line CSV. A cell that writes to a production database becomes one that writes to a temp table.

Fixtures make two things possible:

- **Testing**: run each cell in isolation, with known inputs, and assert on the outputs
- **Playground mode**: spin up a kernel with the full preceding state recreated cheaply, so the AI can experiment with new code against a realistic environment

## User Scenario

A developer has a 3-cell pandas notebook:

- **Cell 1**: Load a large CSV into `df`
- **Cell 2**: Add a `total` column (`price * quantity`)
- **Cell 3**: Plot `df` as a bar chart

The AI generates fixtures for each cell. To test cell 3, it runs the cells cumulatively:

1. **Cell 1's fixture** creates a small temp CSV with known values, registers teardown to delete it, and declares `csv_name` as a global. Cell 1's patched code runs, loading the small CSV into `df`.
2. **Cell 2's fixture** overwrites `df["price"]` and `df["quantity"]` with controlled values. Cell 2's patched code runs, adding the `total` column.
3. **Cell 3** has no fixture. Cell 3's code runs. Assertions check the plot output.
4. **Teardown** fires in reverse order — the temp CSV is deleted.

Each cell's test is cheap and fast. No 3GB file is ever read.

## Metadata Schema

Fixtures are stored in a cell's `ipso` metadata under the `fixtures` key: a dict keyed by fixture name. Each cell has its own `fixtures` dict — fixtures from different cells are separate entries in their respective cell's metadata.

Cell 1's metadata:

```json
{
  "ipso": {
    "fixtures": {
      "load_small_csv": {
        "description": "Creates a 10-line temp CSV for testing the data loading cell",
        "source": "..."
      }
    }
  }
}
```

Cell 2's metadata:

```json
{
  "ipso": {
    "fixtures": {
      "mock_columns": {
        "description": "Overwrites price and quantity columns with known values",
        "source": "..."
      }
    }
  }
}
```

### Fields

- **`description`** _(string, required)_: What the fixture does and its relation to the cell. Used by the AI when deciding whether a fixture needs updating.
- **`priority`** _(integer, optional, default 0)_: Intra-cell execution order. Lower values run first. Fixtures with the same priority run in arbitrary order.
- **`source`** _(string, required)_: The fixture Python source. See [Variable Scoping](#variable-scoping) for conventions.

### Fixture Naming

Fixture names must be globally unique across the notebook. The AI names fixtures descriptively (`load_small_csv`, `mock_db_connection`). If there is a naming conflict, the cell ID is appended (`load_small_csv_abc123`).

## Execution Model

Testing a cell uses a cumulative model: to test cell N, the runner replays the notebook from cell 1 through cell N in order.

For each cell 1 through N:
1. Sort that cell's fixtures by `priority` (ascending)
2. Execute each fixture in order
3. Execute the cell's patched source (or unpatched source if no diff exists)

Then for cell N only:
4. Execute cell N's assertions

This mirrors how a developer would manually run the notebook top-to-bottom, but with fixtures substituting expensive operations at each step.

## Variable Scoping

The runner executes each fixture's `source` as a normal code cell in the test kernel (same namespace as the patched notebook cells). Top-level assignments and `def` statements are visible to the patched cell for that step and to all later cells in the cumulative test run.

You do **not** need `global` for ordinary names you want the cell under test to see:

```python
# fixture source for "load_small_csv":
import tempfile
import os

tmp = tempfile.NamedTemporaryFile(mode="w+", suffix=".csv", delete=False)
with open("huge_file.csv") as f:
    for i, line in enumerate(f):
        if i < 10:
            tmp.write(line)
        else:
            break
tmp.flush()
csv_name = tmp.name

ipso.register_teardown(lambda: os.unlink(tmp.name))
```

After the fixture runs, `csv_name` (and any other names you assign at the top level) remain in the kernel namespace for patched cell code and subsequent fixtures.

**Namespace hygiene:** Unlike a function wrapper, temporary names (e.g. `tmp`) also stay in the kernel until overwritten or the kernel exits. Use `del`, narrow scopes with small helpers, or accept leftover names if harmless.

You can still use `global` when you need to assign into an outer scope from inside a nested function (same rules as ordinary Python).

## Teardown

Fixtures register cleanup callbacks using `ipso.register_teardown(callback)`:

```python
ipso.register_teardown(lambda: os.unlink(tmp.name))
```

Callbacks are stored in a LIFO stack. When the runner finishes a test, it triggers teardown — callbacks fire in reverse registration order, cleaning up in the opposite order from setup.

In **playground mode**, teardown is deferred and fires only on explicit kernel shutdown.

## Worked Example

3-cell notebook: load CSV → add column → plot.

### Cell 1 fixture source (`load_small_csv`)

```python
import tempfile
import os

global csv_name

tmp = tempfile.NamedTemporaryFile(mode="w+", suffix=".csv", delete=False)
with open("sales.csv") as f:
    for i, line in enumerate(f):
        if i < 10:
            tmp.write(line)
        else:
            break
tmp.flush()
csv_name = tmp.name

ipso.register_teardown(lambda: os.unlink(tmp.name))
```

### Cell 2 fixture source (`mock_columns`)

```python
df["price"] = [10.0] * len(df)
df["quantity"] = [2] * len(df)
```

### What the runner executes

```
[cell 1] run fixture load_small_csv (as a code cell)
[cell 1] run patched cell 1: df = pd.read_csv(csv_name)
[cell 2] run fixture mock_columns (as a code cell)
[cell 2] run patched cell 2: df["total"] = df["price"] * df["quantity"]
[cell 3] no fixtures
[cell 3] run cell 3: df.plot(...)
[cell 3] run assertions
[teardown] os.unlink(tmp.name)
[kernel shutdown]
```

### Teardown sequence

Teardown fires in reverse order of registration. `load_small_csv` registered its callback first, so it fires last — after any callbacks registered by later cells.

## Formal Specification

### `ipso.fixtures` schema

```json
{
  "ipso": {
    "fixtures": {
      "<fixture_name>": {
        "description": "<string>",
        "priority": "<integer, optional, default 0>",
        "source": ["<line1>\n", "<line2>\n"]
      }
    }
  }
}
```

#### `fixture_name`

A non-empty string. Fixture names must be **globally unique across all cells in the notebook** — not just within the cell that defines them — so errors and tooling can refer to a single fixture unambiguously. If a conflict arises, the convention is to append the cell ID: `load_small_csv_abc123`.

Valid characters: letters, digits, and underscores. Using a valid Python identifier is recommended for consistency with editor tooling and generated labels.

#### `description`

A plain string describing what the fixture does and its relation to the cell. Required. Used by the AI when evaluating whether a fixture needs updating after cell edits.

#### `priority`

An integer. Optional, default `0`. Controls intra-cell execution order — lower values run first. When two fixtures share the same priority, execution order is undefined. Use priority only when one fixture within the same cell depends on state set by another.

#### `source`

An array of strings, where each string is a line of Python source code ending with `\n` (except optionally the last line). This matches the `cell.source` convention in nbformat 4, and keeps raw notebook JSON human-readable when inspected directly.

Example:

```json
"source": [
  "global csv_name\n",
  "\n",
  "import tempfile\n",
  "tmp = tempfile.NamedTemporaryFile(mode='w+', suffix='.csv', delete=False)\n",
  "csv_name = tmp.name\n"
]
```

The runner joins the array with `""` before execution — no separator is needed since each line already includes its trailing `\n`.

The source runs at the top level of the kernel user namespace. It may call `ipso.register_teardown(callback)` to register cleanup. It must not rely on any state other than what has already been established by prior fixtures or prior cells in the cumulative execution chain.

### Fixture execution

For each fixture, the runner sends the fixture `source` string as a single `execute_request` to the kernel (one notebook code cell in the generated test notebook), with runner metadata recording `fixture_name` for error reporting.

### Execution order algorithm

For each cell in notebook order (1 through N):
1. Collect the cell's fixtures into a list
2. Sort by `priority` ascending (stable sort — equal priorities preserve definition order)
3. For each fixture, send the fixture `source` as an `execute_request` to the kernel
4. Execute the cell's patched source, or unpatched source if no diff exists

For cell N only:
5. Execute assertions
6. Trigger teardown by sending `ipso._runner.run_teardowns()` as an `execute_request`
7. Shut down the kernel
