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

Fixtures are stored in a cell's `nota-bene` metadata under the `fixtures` key: a dict keyed by fixture name. Each cell has its own `fixtures` dict — fixtures from different cells are separate entries in their respective cell's metadata.

Cell 1's metadata:

```json
{
  "nota-bene": {
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
  "nota-bene": {
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

The runner wraps each fixture's `source` in a generated function before sending it to the kernel. This keeps fixture internals scoped to the function and avoids polluting the kernel's global namespace with intermediate variables.

To make a value available to the cell code or subsequent fixtures, the fixture explicitly declares it with `global`:

```python
# fixture source for "load_small_csv":
import tempfile
import os

global csv_name

tmp = tempfile.NamedTemporaryFile(mode="w+", suffix=".csv", delete=False)
with open("huge_file.csv") as f:
    for i, line in enumerate(f):
        if i < 10:
            tmp.write(line)
        else:
            break
tmp.flush()
csv_name = tmp.name

nota_bene.register_teardown(lambda: os.unlink(tmp.name))
```

After the fixture runs, `csv_name` is in the kernel's global namespace and available to the patched cell code and any subsequent fixtures.

Functions work the same way — declaring a function `global` puts it into the kernel namespace:

```python
global normalize_row

def normalize_row(row):
    return row.strip().lower()
```

Any variable or function not declared `global` remains local to the fixture function and is discarded after the fixture runs. This is intentional — fixtures should be explicit about what they expose.

## Teardown

Fixtures register cleanup callbacks using `nota_bene.register_teardown(callback)`:

```python
nota_bene.register_teardown(lambda: os.unlink(tmp.name))
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

nota_bene.register_teardown(lambda: os.unlink(tmp.name))
```

### Cell 2 fixture source (`mock_columns`)

```python
df["price"] = [10.0] * len(df)
df["quantity"] = [2] * len(df)
```

### What the runner executes

```
[cell 1] wrap load_small_csv in function → call → csv_name hoisted to globals via global declaration
[cell 1] run patched cell 1: df = pd.read_csv(csv_name)
[cell 2] wrap mock_columns in function → call
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

### `nota-bene.fixtures` schema

```json
{
  "nota-bene": {
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

A non-empty string. Fixture names must be **globally unique across all cells in the notebook** — not just within the cell that defines them. This is because all fixtures run in the same kernel namespace and share the same function name prefix. If a conflict arises, the convention is to append the cell ID: `load_small_csv_abc123`.

Valid characters: letters, digits, and underscores. Must be a valid Python identifier (it is used as a function name in the wrapping algorithm below).

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

The runner joins the array with `""` before wrapping — no separator is needed since each line already includes its trailing `\n`.

The source may use `global` declarations to promote variables into the kernel namespace. It may call `nota_bene.register_teardown(callback)` to register cleanup. It must not rely on any state other than what has already been established by prior fixtures or prior cells in the cumulative execution chain.

### Fixture wrapping algorithm

The runner generates the following code for each fixture and sends it as a single `execute_request` to the kernel:

```python
def _nb_fixture_{fixture_name}():
{indented_source}

_nb_fixture_{fixture_name}()
```

Where `{indented_source}` is the fixture `source` with every line indented by 4 spaces. The `global` declarations inside the source promote the named variables into the kernel's global namespace automatically — no return value handling or additional hoisting is required.

### Execution order algorithm

For each cell in notebook order (1 through N):
1. Collect the cell's fixtures into a list
2. Sort by `priority` ascending (stable sort — equal priorities preserve definition order)
3. For each fixture, generate the wrapped source and send as an `execute_request` to the kernel
4. Execute the cell's patched source, or unpatched source if no diff exists

For cell N only:
5. Execute assertions
6. Trigger teardown by sending `nota_bene._run_teardowns()` as an `execute_request`
7. Shut down the kernel
