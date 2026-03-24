# Fixture execution, editor guides, and plain tracebacks

This spec records the product decisions implemented for fixture execution, clearer test-editor layout, and traceback output. It aligns with [docs/fixtures.md](../docs/fixtures.md), [specs/cli-test-runner.md](cli-test-runner.md), and [specs/test-editor-cli.md](test-editor-cli.md).

## 1. Fixture source runs as plain kernel cells

Fixture metadata `source` is executed **as-is** in the test kernel (one generated code cell per fixture in the internal test notebook). There is no generated `def _nb_fixture_...()` wrapper.

**Rationale:** A function wrapper made assignments local unless authors added `global`, which was easy to miss and broke common patterns (e.g. `data = [...]` without `global`).

**Tradeoff:** Temporary names in fixture source remain in the kernel namespace for the rest of the test run (same as normal notebook cells). Authors may `del` names or keep harmless temps.

## 2. Test editor: markdown guide cells

The `ipso edit` notebook inserts markdown cells with `ipso.editor.role == "guide"` between:

- Section header and fixture block  
- Fixture block and patched source  
- Patched source and test cell  

Sections without ipso metadata get a guide before the passthrough source cell.

On `edit --continue`, the section parser **skips** `guide` cells (no warning). User-added markdown without this role still triggers the existing “non-code cell … ignored” warning.

## 3. Plain tracebacks: post-processing in Rust

The Jupyter kernel (IPython) may emit ANSI CSI/OSC sequences and other C0 control characters in `error` outputs and in subtest tracebacks embedded in results JSON.

The CLI does **not** set `NO_COLOR` or other terminal-oriented env vars on the executor. Instead, [`extract_results`](../src/test_runner.rs) and [`format_error`](../src/test_runner.rs) run kernel-originated strings through **`sanitize_kernel_text`**, which strips:

- ANSI CSI sequences (`ESC [ … final byte`)
- ANSI OSC sequences (`ESC ] … BEL` or `ESC \`)
- Simple `ESC ( n` / `ESC ) n` character-set selects
- Other ASCII control characters except newline and tab (carriage return is dropped)

Applied to:

- Notebook `error` output: joined traceback and the `ename: evalue` detail line
- Parsed subtest results: `error` and `traceback` fields before building the completed `CellTestResult`

If new escape styles appear in future IPython versions, extend the sanitizer or add a second pass.
