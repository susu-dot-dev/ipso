# Programmatic CLI Access

This spec describes the programmatic CLI interface for driving nota-bene notebook
updates without going through the interactive editor flow. The target audience is
scripts, AI agents, and other tools that need to read and write notebook metadata
without human involvement.

## Design Principles

- All commands read/write JSON. There is no `--json` flag; JSON is the only output
  format for these commands. Users who want a human-readable view should use
  `nb edit` / the `.ipynb` editor flow instead.
- `--stdin` must be passed explicitly to read the notebook from stdin. Without this
  flag, a filename is required. This avoids the command appearing to hang when a
  user forgets to pass a filename.
- In `--stdin` mode, the modified notebook is written to stdout, enabling pipe
  workflows: `cat nb.ipynb | nb update --stdin --data '...' | sponge nb.ipynb`
- In file mode, changes are written back to the same file in place.
- A single diagnostics schema is shared across validation output and input rejection
  errors — callers have one error model to handle.
- SHAs are the sole signal for "this cell has been deliberately reviewed". A cell
  with no SHA has never been accepted; a cell with a valid SHA has been accepted in
  its current state. The three-state `Option<Option<T>>` model does not appear in
  the external API — fields are either `null` or have a value.

## Commands

### `nb view`

Read cell metadata as JSON.

```
nb view <path> [--stdin] [--filter "key:expr"...] [--fields f1,f2,...]
```

Always outputs a JSON array of cell objects, even when a single cell matches.
This makes it easy to compose with other tools without special-casing the output.

`--stdin`: reads the notebook from stdin. `<path>` is still required as a hint
for error messages but is not read from disk.

### `nb update`

Apply a JSON blob of changes to one or more cells.

```
nb update <path> [--stdin] (--data <json> | --data-file <path>)
```

- `--data <json>`: inline JSON string (callers can use `--data "$(cat file.json)"`)
- `--data-file <path>`: read the blob from a file
- One of `--data` or `--data-file` is required.
- The blob can be a single cell object or an array of cell objects.
- If validation of the input blob fails, the command exits non-zero and prints
  a diagnostics object to stderr without modifying the notebook.

### `nb status`

Alias for:

```
nb view <path> [--stdin] [--filter "key:expr"...] \
    --filter "status.valid:false" \
    --fields cell_id,status
```

Plus exits non-zero if any cells are returned (i.e. any cell is invalid).

Useful for CI: `nb status notebook.ipynb && echo "all good"`.

Additional `--filter` flags narrow which cells are considered before the
`status.valid:false` filter is applied.

### `nb accept`

Recompute and store the SHA snapshot for cells, marking them as up-to-date.

```
nb accept <path> [--stdin] [--filter "key:expr"...]
```

- Without any filter: accepts all cells.
- With filters: accepts only matching cells.
- In `--stdin` mode: writes the updated notebook to stdout.
- In file mode: writes back to the file in place.

### `nb scaffold`

Generate a well-formed JSON fragment for use with `nb update`. Useful as a
starting point when constructing update blobs programmatically.

```
nb scaffold fixture --name <name> --description <desc> --priority <n> --source <src>
nb scaffold test --name <name> --source <src>
```

Outputs a JSON fragment to stdout. Does not read or write any notebook file.

Example:

```
nb scaffold fixture \
  --name setup_df \
  --description "Small test dataframe" \
  --priority 1 \
  --source "global df; df = pd.DataFrame({'amount': [1, 2, 3]})"
```

Output:

```json
{
  "fixtures": {
    "setup_df": {
      "description": "Small test dataframe",
      "priority": 1,
      "source": "global df; df = pd.DataFrame({'amount': [1, 2, 3]})"
    }
  }
}
```

---

## Filter System

All commands that read cells support `--filter` and `--fields`.

### `--filter "key:expr"`

Selects which cells to include. Multiple `--filter` flags combine with AND
(intersection). Comma-separated values within a single filter combine with OR
(union).

```
# AND: stale cells that also have no test
--filter "diagnostics.type:stale" --filter "test:null"

# OR within a filter: stale or diff_conflict
--filter "diagnostics.type:stale,diff_conflict"
```

Without any `--filter`, all code cells are returned.

Filter keys use dot-notation into the cell object:

| Key | Values | Description |
|-----|--------|-------------|
| `cell` | `<id>[,<id>...]` | Match specific cell IDs |
| `index` | `n`, `n..m`, `n..`, `..m` | Match cells by 0-based position |
| `test` | `null`, `not null` | Test is absent or present |
| `fixtures` | `null`, `not null` | Fixtures are absent or present |
| `diff` | `null`, `not null` | Diff is absent or present |
| `status.valid` | `true`, `false` | Overall validity |
| `diagnostics.type` | `<type>[,<type>...]` | Has a diagnostic of this type |
| `diagnostics.severity` | `error`, `warning` | Has a diagnostic of this severity |

The filter key space is the cell object's JSON structure. New filter keys can be
added in future without breaking existing callers.

### `--fields f1,f2,...`

Controls which fields appear on each cell object in the output. `cell_id` is
always included regardless. Default is all fields.

```
# Only return cell_id and status (compact view)
nb view notebook.ipynb --fields cell_id,status

# Only source and test
nb view notebook.ipynb --fields source,test
```

---

## JSON Schemas

### Cell object (view output / update input)

```json
{
  "cell_id": "compute-total",
  "source": "total = df['amount'].sum()\nprint(total)",
  "fixtures": {
    "setup_df": {
      "description": "Small test dataframe",
      "priority": 1,
      "source": "global df\ndf = pd.DataFrame({'amount': [1, 2, 3]})"
    }
  },
  "diff": "--- a/cell\n+++ b/cell\n@@ -1 +1 @@\n-df.read_csv('huge.csv')\n+df",
  "test": {
    "name": "test_total",
    "source": "nota_bene.execute_cell()\nassert total == 6"
  },
  "status": {
    "valid": false,
    "diagnostics": [ ... ]
  }
}
```

**Fields:**

| Field | In view? | In update? | Notes |
|-------|----------|------------|-------|
| `cell_id` | yes | yes (required) | Jupyter cell ID |
| `source` | yes | ignored | Read-only; the raw cell source |
| `fixtures` | yes | yes | `null` or a map of fixture objects |
| `diff` | yes | yes | `null` or a unified diff string |
| `test` | yes | yes | `null` or a test object |
| `status` | yes | ignored | Output only |

`shas` are never exposed in the JSON interface. They are an internal implementation
detail. Whether a cell has been accepted is surfaced through `status.diagnostics`
(a `missing_sha` diagnostic means never accepted) and `nb accept`.

### Field semantics

`fixtures`, `diff`, and `test` are two-state in the external API:

| JSON | Meaning in view output | Meaning in update input |
|------|------------------------|-------------------------|
| `null` | No value (not set) | Set to null (clear) |
| value | Has content | Set to this value |

The internal three-state representation (`Option<Option<T>>`) is an implementation
detail and does not appear in the API. The question of "has anyone deliberately
reviewed this cell" is answered entirely by the SHA: if the cell has been accepted
(`nb accept`), its current state is intentional. If not, `missing_sha` appears
in diagnostics.

### Fixture map merge semantics

When `fixtures` is present in an update blob, it is **merged** with existing
fixtures at the key level:

| Fixture key value | Meaning |
|-------------------|---------|
| object | Upsert this fixture (create or update) |
| `null` | Remove this specific fixture |
| absent | Leave this fixture unchanged |

To clear all fixtures at once, set `fixtures` itself to `null`.

### Diagnostics object

Used as both the `status` field in view output and the error response from
`nb update` when input is rejected. Callers have one error model for both.

```json
{
  "valid": false,
  "diagnostics": [
    {
      "type": "missing_sha",
      "severity": "warning",
      "message": "cell has never been accepted",
      "field": "shas"
    },
    {
      "type": "stale",
      "severity": "error",
      "message": "cell was modified since it was last accepted",
      "field": "shas"
    },
    {
      "type": "diff_conflict",
      "severity": "error",
      "message": "diff does not apply cleanly to current cell source",
      "field": "diff"
    },
    {
      "type": "missing_field",
      "severity": "error",
      "message": "fixture 'setup_df' is missing required field 'description'",
      "field": "fixtures.setup_df.description"
    }
  ]
}
```

**`valid`** is `true` if and only if `diagnostics` is empty.

**Diagnostic fields:**

| Field | Type | Description |
|-------|------|-------------|
| `type` | string enum | Machine-readable diagnostic type |
| `severity` | `"error"` \| `"warning"` | Errors block validity; warnings are informational |
| `message` | string | Human-readable description |
| `field` | string | Dot-notation path to the affected field |

**Diagnostic types:**

| Type | Severity | Description |
|------|----------|-------------|
| `missing_sha` | warning | Cell has never been accepted |
| `stale` | error | Cell or upstream cells modified since last `nb accept` |
| `diff_conflict` | error | Stored diff does not apply cleanly to current cell source |
| `missing_field` | error | Required field absent in update input |
| `invalid_value` | error | Field value is the wrong type or out of range |
| `unknown_cell` | error | `cell_id` in update blob does not exist in the notebook |

---

## Examples

### View all cells

```
nb view notebook.ipynb
```

### View a specific cell

```
nb view notebook.ipynb --filter "cell:compute-total"
```

### View a range of cells

```
nb view notebook.ipynb --filter "index:2..4"
nb view notebook.ipynb --filter "index:2.."
```

### View cells that have never been accepted

```
nb view notebook.ipynb --filter "diagnostics.type:missing_sha"
```

### View cells without a test

```
nb view notebook.ipynb --filter "test:null"
```

### View cells with errors

```
nb view notebook.ipynb --filter "diagnostics.severity:error" --fields cell_id,status
```

### Combine filters

```
# Stale cells that also have no test (AND)
nb view notebook.ipynb --filter "diagnostics.type:stale" --filter "test:null"

# Stale or diff_conflict (OR within filter)
nb view notebook.ipynb --filter "diagnostics.type:stale,diff_conflict"

# First 5 cells with no fixtures
nb view notebook.ipynb --filter "index:..4" --filter "fixtures:null"
```

### Update a test

```
nb update notebook.ipynb --data '{
  "cell_id": "compute-total",
  "test": {
    "name": "test_total",
    "source": "nota_bene.execute_cell()\nassert total == 6"
  }
}'
```

### Remove a specific fixture

```
nb update notebook.ipynb --data '{
  "cell_id": "compute-total",
  "fixtures": { "old_fixture": null }
}'
```

### Clear all fixtures

```
nb update notebook.ipynb --data '{
  "cell_id": "compute-total",
  "fixtures": null
}'
```

### Batch update across multiple cells

```
nb update notebook.ipynb --data-file changes.json
```

Where `changes.json` is an array:

```json
[
  {
    "cell_id": "load-data",
    "fixtures": {
      "setup_df": {
        "description": "Small test dataframe",
        "priority": 1,
        "source": "global df\ndf = pd.DataFrame({'amount': [1, 2, 3]})"
      }
    }
  },
  {
    "cell_id": "compute-total",
    "test": {
      "name": "test_total",
      "source": "nota_bene.execute_cell()\nassert total == 6"
    }
  }
]
```

### Accept all cells

```
nb accept notebook.ipynb
```

### Accept only stale cells

```
nb accept notebook.ipynb --filter "diagnostics.type:stale"
```

### Validate (CI usage)

```
nb status notebook.ipynb && echo "all good"
```

### Validate specific cells

```
nb status notebook.ipynb --filter "cell:compute-total,load-data"
```

### Pipe workflow

```
cat notebook.ipynb | nb update --stdin --data-file changes.json | sponge notebook.ipynb
```

### Using nb scaffold

```
nb scaffold fixture \
  --name setup_df \
  --description "Small test dataframe" \
  --priority 1 \
  --source "$(cat setup_df.py)" | \
jq --arg cell compute-total '{cell_id: $cell} + .' | \
nb update notebook.ipynb --data-file /dev/stdin
```

---

## Relationship to the interactive editor flow

`nb edit`, `nb edit --continue`, and `nb edit --clean` remain unchanged and are
the recommended interface for humans working interactively. The programmatic
commands (`nb view`, `nb update`, `nb status`, `nb accept`) are a separate
interface for scripts and AI agents.

The underlying data model is identical. The programmatic commands operate directly
on the notebook metadata; the editor flow is a higher-level workflow that
materialises metadata as visible notebook cells and then collapses them back.
