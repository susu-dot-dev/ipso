# MCP Repair Tool

## Overview

A single MCP tool, `repair_ipso`, that acts as a state-machine-driven
repair loop for notebook cell metadata. Each invocation identifies the next
cell needing attention, runs its test if one exists, and returns a
context-rich prompt that instructs the calling AI agent to fix the cell
using the ipso CLI.

The tool never writes to the notebook. The AI agent performs all writes
via `ipso update` and `ipso accept` CLI commands, then calls
`repair_ipso` again to advance to the next cell.

## Motivation

ipso attaches test infrastructure (fixtures, diffs, tests) to each
notebook cell so that hundreds of parallel kernels can quickly recreate
notebook state and run tests. This requires:

- **Fixtures**: Deterministic mock data and controlled side effects.
  Lightweight (KB not GB) so kernels spin up fast. Fixtures model the
  shape of real data but are quick to load and exercise specific cases.
  NOT needed if data was initialized by a previous cell or the cell has
  no external side effects.

- **Diffs**: Minimal unified patches that reroute cell source code to use
  fixture-provided resources (e.g., swap a CSV path for a temp file).
  The goal is to test the user's actual code as much as possible —
  patching should be as small as possible.

- **Tests**: Unit-test-style Python code using `ipso.subtest()` that
  verifies the cell's behavior across multiple conditions and edge cases.

The MCP tool automates the process of guiding an AI agent through creating
and maintaining this metadata, providing cell-specific context and
instructions tailored to each diagnostic state.

## Tool Interface

### Name

`repair_ipso`

### Parameters

```json
{
  "notebook_path": "string (required) — path to the .ipynb file",
  "cell_id": "string (optional) — target a specific cell; if omitted, the first cell needing repair is selected",
  "detail_level": "object (optional) — per-diagnostic-type verbosity override"
}
```

### `detail_level` schema

An object mapping diagnostic type names to `"brief"` or `"detailed"`.
Diagnostic type names match the LSP server's diagnostic types exactly:

- `missing`
- `needs_review`
- `ancestor_modified`
- `diff_conflict`
- `invalid_field`

Default: all types are `"detailed"`.

Example:
```json
{
  "detail_level": {
    "missing": "brief",
    "needs_review": "detailed"
  }
}
```

Omitted keys default to `"detailed"`.

In **detailed** mode, the response includes full instructional prose
explaining what fixtures, diffs, and tests are, why they're needed, and
exact CLI commands with populated values.

In **brief** mode, the response includes the same cell context (source,
existing metadata, test results) and the same exact CLI commands, but
replaces the surrounding instructional prose with a one-line summary
and a note to call with `"detailed"` for full explanations.

## Internal Logic

The tool is read-only — it never modifies the notebook.

### Steps

1. Load the notebook via `notebook::load_notebook(&path)`.
2. Compute the SHA snapshot via `shas::compute_snapshot(&nb)`.
3. For each code cell, compute:
   - Cell state via `shas::cell_state(&nb, &snapshot, cell_index)`
   - Own diagnostics via `diagnostics::compute_own_diagnostics(cell)`
4. Select the target cell:
   - If `cell_id` is provided, use that cell (error if not found or valid).
   - Otherwise, pick the first code cell with non-empty diagnostics.
5. If no cell needs repair, return the "all clear" response.
6. If the target cell has an existing test, run it:
   - Shell out to `<current_exe> test <notebook_path> --filter cell:<cell_id> --timeout 30`
   - Parse stdout as `Vec<CellTestResult>` JSON.
   - If the cell has no test, note "No test defined for this cell."
7. Read the cell's source and existing ipso metadata (fixtures, diff, test).
8. Determine which diagnostic types apply to this cell.
9. Build the response using the appropriate template for each diagnostic type,
   at the detail level specified (or defaulting to detailed).

### Test execution

Tests are run by shelling out to the current binary:

```rust
let exe = std::env::current_exe()?;
let output = tokio::process::Command::new(&exe)
    .args(["test", &notebook_path, "--filter", &format!("cell:{}", cell_id), "--timeout", "30"])
    .output()
    .await?;
```

The JSON output is parsed to extract pass/fail/error information.
If the process exits with code 2 (infrastructure error) or fails to
spawn, the error is included in the response.

## Response Format

Every response includes:

1. **Header**: Cell ID, index, notebook path, and a summary of what's wrong.
2. **Cell source**: The full current source of the cell.
3. **Existing metadata** (if any): Fixtures, diff, test.
4. **Test result** (if a test exists): Pass/fail/error details.
5. **CLI commands**: Exact `ipso update` and `ipso accept` commands
   with all values populated. Present in BOTH detailed and brief modes.
6. **Instructions** (detailed only): Full explanation of fixtures, diffs, tests.
7. **Loop instruction**: "Then call `repair_ipso` again to continue
   to the next cell."

### All cells valid

```
All cells in <notebook_path> are valid. Nothing to repair.
```

### Diagnostic: `missing` (detailed)

```
Cell `<cell_id>` at index <N> in `<notebook_path>` has no ipso
metadata and needs fixtures, a diff, and tests.

## Cell source

\```python
<cell source verbatim>
\```

## Instructions

ipso attaches test infrastructure to each cell so that hundreds of
parallel kernels can quickly recreate the notebook state up to any cell
and run isolated tests.

### 1. Fixtures

Fixtures create deterministic mock data and controlled side effects that
replace real resources (files, databases, APIs). They must be lightweight
(KB not GB) so test kernels spin up fast.

A fixture is NOT needed if:
- The cell's data was already initialized by a previous cell
- The cell has no external side effects or resource dependencies

Each fixture has a name (the key), and an object with:
- `description`: What this fixture sets up and why
- `priority`: Integer (lower runs first)
- `source`: Python code that creates the mock data/resources

### 2. Diff

A minimal unified diff that patches the cell source to use fixtures
instead of real resources. For example, replacing a CSV file path with
a temp file created by the fixture.

The diff should change as little as possible — the goal is to test the
user's actual code, not rewritten code. A diff is NOT needed if the
cell doesn't reference any external resources that fixtures replace.

### 3. Test

Python code with subtests that verify the cell behaves correctly. Use
`ipso.subtest("name")` as a context manager to define each subtest.
Each subtest should cover a specific condition or edge case the cell
handles.

### Commands

\```bash
ipso update <notebook_path> --data '{
  "cell_id": "<cell_id>",
  "fixtures": {
    "<fixture_name>": {
      "description": "<what this fixture sets up>",
      "priority": 0,
      "source": "<python code>"
    }
  },
  "diff": "<unified diff string, or omit entirely if not needed>",
  "test": {
    "name": "<descriptive_test_name>",
    "source": "<python test code using ipso.subtest()>"
  }
}'
\```

If no fixtures or diff are needed, omit those fields entirely and only
provide the test.

After writing metadata:

\```bash
ipso accept <notebook_path> --filter cell:<cell_id>
\```

Then call `repair_ipso` again to continue to the next cell.

On subsequent calls, you may pass `detail_level` with `"brief"` for
diagnostic types you've already seen, to reduce context. For example:
`detail_level: {"missing": "brief"}`
```

### Diagnostic: `missing` (brief)

```
Cell `<cell_id>` at index <N> in `<notebook_path>` has no ipso metadata.

## Cell source

\```python
<cell source>
\```

## Commands

\```bash
ipso update <notebook_path> --data '{
  "cell_id": "<cell_id>",
  "fixtures": { ... },
  "diff": "...",
  "test": { "name": "...", "source": "..." }
}'
ipso accept <notebook_path> --filter cell:<cell_id>
\```

Create fixtures (if external resources/side effects are involved), a
minimal diff (if needed to reroute to fixtures), and tests for this cell.
Call with `detail_level: {"missing": "detailed"}` for full instructions.
Then call `repair_ipso` again.
```

### Diagnostic: `needs_review` (detailed)

```
Cell `<cell_id>` at index <N> in `<notebook_path>` has changed since its
ipso metadata was last accepted.

## Cell source (current)

\```python
<cell source>
\```

## Existing fixtures

<JSON of fixtures map, or "None">

## Existing diff

\```diff
<diff string, or "None">
\```

## Existing test

Name: <test_name>
\```python
<test source>
\```

## Test result

<formatted pass/fail/error>

## Instructions

The cell source or metadata has changed since the last accept. Review
whether the existing fixtures, diff, and test still make sense for the
current cell source.

Remember:
- Fixtures provide deterministic mock data (KB not GB)
- The diff should be minimal — only reroute to use fixtures
- Tests should cover the specific behaviors of THIS cell

<if test passed>
The test still passes. If the metadata looks correct for the current
source, just accept:

\```bash
ipso accept <notebook_path> --filter cell:<cell_id>
\```

<if test failed or errored>
The test is failing. Examine what changed and update as needed:

\```bash
ipso update <notebook_path> --data '{
  "cell_id": "<cell_id>",
  <only include fields that need changing>
}'
ipso accept <notebook_path> --filter cell:<cell_id>
\```

Then call `repair_ipso` again to continue to the next cell.

On subsequent calls, you may pass `detail_level` with `"brief"` for
diagnostic types you've already seen.
```

### Diagnostic: `needs_review` (brief)

```
Cell `<cell_id>` at index <N> in `<notebook_path>` has changed since last accept.

## Cell source (current)

\```python
<cell source>
\```

## Existing metadata

Fixtures: <JSON or "None">
Diff: <diff or "None">
Test: <test source>

## Test result

<pass/fail/error>

## Commands

<if test passed>
\```bash
ipso accept <notebook_path> --filter cell:<cell_id>
\```

<if test failed>
\```bash
ipso update <notebook_path> --data '{
  "cell_id": "<cell_id>",
  <fields to update>
}'
ipso accept <notebook_path> --filter cell:<cell_id>
\```

Review and update or accept. Call with `detail_level: {"needs_review": "detailed"}`
for full instructions. Then call `repair_ipso` again.
```

### Diagnostic: `ancestor_modified` (detailed)

```
Cell `<cell_id>` at index <N> in `<notebook_path>` has ancestor cells
that were modified.

Changed ancestors: <comma-separated cell IDs>

## Cell source

\```python
<cell source>
\```

## Existing fixtures

<JSON or "None">

## Existing diff

\```diff
<diff or "None">
\```

## Existing test

Name: <test_name>
\```python
<test source>
\```

## Test result

<pass/fail/error>

## Instructions

Preceding cells have changed, which may affect this cell's execution
context. The fixtures and diff may need updating if the upstream data
shape or control flow changed.

<if test passed>
The test still passes despite ancestor changes. If the upstream changes
don't affect this cell's behavior, just accept:

\```bash
ipso accept <notebook_path> --filter cell:<cell_id>
\```

<if test failed or errored>
The test is failing, likely due to changes in preceding cells. Update
the fixtures or test to account for the new upstream state:

\```bash
ipso update <notebook_path> --data '{
  "cell_id": "<cell_id>",
  <fields to update>
}'
ipso accept <notebook_path> --filter cell:<cell_id>
\```

Then call `repair_ipso` again to continue to the next cell.
```

### Diagnostic: `ancestor_modified` (brief)

```
Cell `<cell_id>` at index <N> in `<notebook_path>` has modified ancestors.
Changed: <cell IDs>

## Cell source

\```python
<cell source>
\```

## Existing metadata

Fixtures: <JSON or "None">
Diff: <diff or "None">
Test: <test source>

## Test result

<pass/fail/error>

## Commands

<if passed>
\```bash
ipso accept <notebook_path> --filter cell:<cell_id>
\```

<if failed>
\```bash
ipso update <notebook_path> --data '{
  "cell_id": "<cell_id>",
  <fields to update>
}'
ipso accept <notebook_path> --filter cell:<cell_id>
\```

Review and update or accept. Call with `detail_level: {"ancestor_modified": "detailed"}`
for full instructions. Then call `repair_ipso` again.
```

### Diagnostic: `diff_conflict` (detailed)

```
Cell `<cell_id>` at index <N> in `<notebook_path>` has a diff that no
longer applies cleanly to the cell source.

## Cell source (current)

\```python
<cell source>
\```

## Stored diff (broken)

\```diff
<the diff that fails>
\```

## Existing fixtures

<JSON or "None">

## Existing test

Name: <test_name>
\```python
<test source>
\```

## Instructions

The cell source has changed and the stored diff can no longer be applied.
Write a new minimal diff that patches the current source to use the
fixtures, or set it to null if a diff is no longer needed.

The diff's only purpose is to reroute code to use fixture-provided
resources (e.g., swap a file path, redirect an API call to a mock).
It should change as little as possible.

\```bash
ipso update <notebook_path> --data '{
  "cell_id": "<cell_id>",
  "diff": "<new unified diff, or null to remove>"
}'
ipso accept <notebook_path> --filter cell:<cell_id>
\```

Then call `repair_ipso` again to continue to the next cell.
```

### Diagnostic: `diff_conflict` (brief)

```
Cell `<cell_id>` at index <N> in `<notebook_path>` has a broken diff.

## Cell source

\```python
<cell source>
\```

## Stored diff (broken)

\```diff
<diff>
\```

## Commands

\```bash
ipso update <notebook_path> --data '{
  "cell_id": "<cell_id>",
  "diff": "<new unified diff, or null to remove>"
}'
ipso accept <notebook_path> --filter cell:<cell_id>
\```

Rewrite the diff for the current source or remove it. Then accept.
Call with `detail_level: {"diff_conflict": "detailed"}` for full instructions.
Then call `repair_ipso` again.
```

### Diagnostic: `invalid_field` (detailed)

```
Cell `<cell_id>` at index <N> in `<notebook_path>` has invalid ipso
metadata.

## Cell source

\```python
<cell source>
\```

## Current metadata (raw)

<raw JSON of the ipso metadata>

## Instructions

One or more ipso metadata fields have validation errors. Fix the
invalid fields using the update command.

Valid field formats:
- `fixtures`: object keyed by name, each with `description` (string),
  `priority` (integer), `source` (string)
- `diff`: string (unified diff) or null
- `test`: object with `name` (string) and `source` (string), or null

\```bash
ipso update <notebook_path> --data '{
  "cell_id": "<cell_id>",
  <corrected fields>
}'
ipso accept <notebook_path> --filter cell:<cell_id>
\```

Then call `repair_ipso` again to continue to the next cell.
```

### Diagnostic: `invalid_field` (brief)

```
Cell `<cell_id>` at index <N> in `<notebook_path>` has invalid metadata.

## Current metadata (raw)

<raw JSON>

## Commands

\```bash
ipso update <notebook_path> --data '{
  "cell_id": "<cell_id>",
  <corrected fields>
}'
ipso accept <notebook_path> --filter cell:<cell_id>
\```

Fix the invalid fields. Call with `detail_level: {"invalid_field": "detailed"}`
for format reference. Then call `repair_ipso` again.
```

### Combined diagnostics

When a cell has multiple diagnostic types (e.g., `needs_review` +
`diff_conflict`), the response includes sections for each, concatenated.
The header lists all active diagnostic types. Instructions address all
issues before accepting.

## Repair Loop

The calling AI is expected to:

1. Call `repair_ipso(notebook_path)` — gets first broken cell
2. Follow instructions: run `ipso update` and `ipso accept`
3. Call `repair_ipso(notebook_path)` again — gets next broken cell
4. Repeat until response is "All cells are valid."

On subsequent iterations, the AI may pass `detail_level` with `"brief"`
for diagnostic types it has already handled, to reduce context usage.

## Tool: `generate_diff`

### Motivation

Generating a correct unified diff is error-prone for AI models. The format
requires exact line numbers and verbatim context lines — off-by-one errors
silently produce diffs that fail to apply. It is far easier for AI to
produce the intended **patched cell source** (the full source after the
desired changes) and have the tool compute the diff.

`repair_ipso` instructions tell the AI: "produce the patched source,
then call `generate_diff` to get the unified diff string".

### Name

`generate_diff`

### Parameters

```json
{
  "notebook_path": "string (required) — path to the .ipynb file",
  "cell_id":       "string (required) — cell to diff against",
  "patched_source": "string (required) — full intended source after patching"
}
```

### Behavior

1. Load the notebook.
2. Find the cell by `cell_id`. Error if not found or not a code cell.
3. Compute a unified diff between the current cell source and `patched_source`
   using `diff_utils::compute_diff`.
4. If the sources are identical, return a message saying no diff is needed.
5. Otherwise return the unified diff string, ready to pass directly to
   `ipso update --data '{"cell_id": "...", "diff": "<returned string>"}'`.

### Usage in repair loop

When `repair_ipso` indicates a diff is needed (Missing or DiffConflict
cases), the AI should:

1. Decide what minimal changes are required to reroute the cell to use
   fixtures instead of real resources.
2. Produce the full patched cell source with those changes applied.
3. Call `generate_diff(notebook_path, cell_id, patched_source)`.
4. Use the returned diff string in the `ipso update` command.

## CLI: `ipso docs`

A `help` subcommand provides detailed reference documentation for topics
that are relevant to both humans and AI agents.

### Usage

```
ipso docs [<topic>]
```

Running with no topic lists available topics. Running with a topic prints
comprehensive documentation including syntax, all options, and examples.

### Available topics

| Topic | Description |
|---|---|
| `filters` | Full filter syntax for `--filter` flags used in `view`, `status`, `accept`, and `test` |

### `ipso docs filters` content

Documents all filter keys:

- `cell:<id>[,<id>,...]` — match by cell ID
- `index:<n|n..m|n..|..m>` — match by 0-based position (range syntax)
- `test:<null|not_null>` — presence of test metadata
- `fixtures:<null|not_null>` — presence of fixtures metadata
- `diff:<null|not_null>` — presence of diff metadata
- `status.valid:<true|false>` — overall validity
- `diagnostics.type:<type>[,...]` — by diagnostic type (missing, needs_review, ancestor_modified, diff_conflict, invalid_field)
- `diagnostics.severity:<error|warning>` — by diagnostic severity

Includes notes on AND/OR combination semantics, behaviour with cell types,
and multiple worked examples.

### References to `help filters`

All commands that accept `--filter` flags reference `ipso docs filters`
in their `--help` output. The `repair_ipso` MCP tool includes the
reference at the bottom of every response. MCP tool descriptions also
include the reference.

## File Changes

### `src/main.rs`

- Add `Help { topic: Option<String> }` variant to the `Command` enum.
- Add `run_help(topic)` function with `HELP_TOPICS` and `HELP_FILTERS`
  static string constants.
- Update `--filter` doc strings on `view`, `status`, `accept`, and `test`
  to reference `ipso docs filters`.

### `src/mcp.rs`

Full rewrite. Remove existing `greet` and `ask_host` placeholder tools.
Implement `repair_ipso` and `generate_diff` as described above.
Imports from existing modules: `notebook`, `metadata`, `diagnostics`,
`diff_utils`. All tool descriptions and every response footer reference
`ipso docs filters`.

### `Cargo.toml`

Add `"process"` feature to the `tokio` dependency for subprocess execution.

## Future Work

- **Sampling support**: When MCP hosts implement `sampling/createMessage`,
  the tool can optionally perform inference internally rather than
  returning a prompt for the calling AI to follow. The external tool
  interface remains the same.

- **Previous cell context**: Currently no preceding cell sources are
  included in the response. A future `include_context: bool` parameter
  could add them for complex notebooks where the AI needs more context.

- **Parallel repair**: Currently the tool returns one cell at a time.
  A future `batch: true` parameter could return all broken cells at
  once for parallel fixing.

- **Additional help topics**: `fixtures`, `metadata`, `workflow` topics
  could be added to `ipso docs` as the project matures.

