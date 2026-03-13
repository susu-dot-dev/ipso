# Staleness Tracking

## The Problem

A notebook is a living document. Cells get edited, reordered, inserted, and deleted — often in bursts, mid-thought, before anything is validated. The AI needs to know: after a round of edits, which cells' fixtures and tests might no longer reflect reality?

The goal is not to automatically fix anything. It's to give the AI a precise, cheap answer to "what should I look at?" so it can make informed decisions rather than scanning the entire notebook every time.

## User Scenario

A developer is working on a data pipeline notebook. The AI has previously generated fixtures and tests for all cells. The developer then:

1. Edits cell 3 to use a different CSV parsing strategy
2. Inserts a new normalization cell between cells 4 and 5
3. Swaps cells 6 and 7 to reorder the pipeline steps

They haven't broken anything intentionally — but they're not sure what downstream tests are now stale. They ask the AI to check.

The AI calls `keep_updated`. Rather than re-reading every cell from scratch, it compares the current notebook structure against each cell's stored `shas` snapshot. This immediately tells it:

- Cells 4 through the end are potentially affected by the edit to cell 3
- The new cell between 4 and 5 is unknown territory — anything after it was validated without it existing
- Cells after the swap of 6 and 7 were validated assuming a different order

The AI now has a focused list. It reads the diffs for the changed cells, inspects the fixtures and tests of the flagged cells, and decides what actually needs updating versus what happens to be unaffected despite the structural change.

## How It Works

Each cell stores a `shas` subkey in its `nota-bene` metadata: an ordered snapshot of every cell from the first through itself, as they were when the cell's fixtures were last validated.

```json
{
  "nota-bene": {
    "fixtures": { ... },
    "diff": "...",
    "test": { ... },
    "shas": [
      {"cell_id": "abc", "sha": "a1b2c3"},
      {"cell_id": "def", "sha": "d4e5f6"},
      {"cell_id": "ghi", "sha": "g7h8i9"}
    ]
  }
}
```

The SHA for each entry covers that cell's content, fixture source, and patch combined — anything that could affect test behavior.

When `keep_updated` runs, it computes the current snapshot of the notebook and compares it against each cell's stored `shas`. A cell is flagged as potentially stale if anything before it (or itself) has changed:

- **Content change**: a SHA no longer matches for a known cell ID
- **Insertion**: a cell ID appears in the current notebook that isn't in the stored snapshot
- **Deletion**: a cell ID in the stored snapshot no longer exists in the notebook
- **Reordering**: the sequence of cell IDs has changed

## What the AI Does With This

The flagged list is a starting point, not a verdict. The AI:

1. Reviews the diffs of cells that actually changed content
2. For each flagged downstream cell, reads its `fixtures` and `test` alongside the diff to decide if the change is actually relevant
3. Updates fixtures or tests where needed, or marks them as still valid
4. Rewrites `shas` for all affected cells to reflect the current validated state

## Multi-Step Edits

Because the check is on-demand, the developer can freely make multiple edits before triggering `keep_updated`. There are no incremental staleness warnings mid-edit — the AI gets a clean, complete picture of all changes at once when it's time to validate.

## Formal Specification

### SHA Computation

For each cell, construct the following object:

```python
{
    "source": cell["source"],
    "nota-bene": {k: v for k, v in cell["metadata"].get("nota-bene", {}).items() if k != "shas"}
}
```

If the cell has no `nota-bene` metadata, the `nota-bene` value is an empty dict `{}`. The `shas` key is excluded to avoid a circular dependency — the hash must be computable before `shas` is written.

Serialize to canonical JSON (deterministic key ordering, no whitespace):

```python
import hashlib
import json

canonical = json.dumps(obj, sort_keys=True, separators=(",", ":"))
sha = hashlib.sha1(canonical.encode("utf-8")).hexdigest()
```

### `shas` Array

Each cell's `nota-bene` metadata contains a `shas` key: an ordered array of dicts representing the notebook's cell ordering at the time the cell was last validated. The array contains one entry per cell from the first cell through the current cell (inclusive), **in notebook order at validation time**:

```json
"shas": [
  {"cell_id": "<jupyter cell id>", "sha": "<40-char sha1 hex digest>"},
  {"cell_id": "<jupyter cell id>", "sha": "<40-char sha1 hex digest>"}
]
```


### Staleness Check

To check whether a cell is stale:

1. Compute the current SHA for every cell in the notebook using the algorithm above
2. Build the current sequence: `[{"cell_id": id, "sha": sha}, ...]` from cell 1 through the cell being checked
3. Compare against the cell's stored `shas` array

A cell is flagged as potentially stale if the stored and current sequences differ in any way:

- Different length (insertion or deletion)
- Different cell ID at any position (reordering, insertion, or deletion)
- Different SHA for any cell ID (content, fixture, or patch change)
