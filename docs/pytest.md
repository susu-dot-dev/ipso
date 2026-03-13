# pytest Runner

## Overview

The `nota-bene` package includes a pytest plugin that discovers and runs notebook cell tests automatically. When pytest encounters a `.ipynb` file, the plugin reads each cell's `nota-bene` metadata, starts a Jupyter kernel, runs the cumulative fixture chain, executes the test code, and maps the results back to pytest's collection and reporting system.

The plugin is part of the same `nota-bene` package that provides the in-kernel library — installing one gives you both.

## User Experience

```
pip install nota-bene
pytest
```

No configuration needed. pytest discovers `.ipynb` files alongside regular Python test files and reports them in the same output:

```
test_utils.py::test_helper PASSED
my_notebook.ipynb::loads csv data PASSED
my_notebook.ipynb::validates price calculation::price=10.0 quantity=2 PASSED
my_notebook.ipynb::validates price calculation::price=0.0 quantity=5 FAILED
my_notebook.ipynb::validates price calculation::price=99.9 quantity=1 PASSED
analysis.ipynb::trains model PASSED
```

The hierarchy is: `notebook filename` :: `test name` :: `subtest name`. For simple tests with no subtests, the subtest level is omitted.

## Plugin Registration

The plugin is registered via a `pytest11` entry point in `pyproject.toml`. pytest scans all installed packages for this entry point on startup and loads matching plugins automatically:

```toml
[project.entry-points."pytest11"]
nota-bene = "nota_bene.pytest_plugin"
```

The plugin module is `nota_bene/pytest_plugin.py` and contains the collection hooks and test item classes.

## Collection Hierarchy

Notebook tests map to a three-level pytest collection hierarchy:

```
NotaBeneNotebook (Collector)      ← one per .ipynb file
  NotaBeneCellTest (Collector)    ← one per cell with nota-bene.test
    NotaBeneSubtest (Item)        ← one per subtest result
```

### `pytest_collect_file` hook

The entry point into pytest's collection. Claims `.ipynb` files:

```python
def pytest_collect_file(parent, file_path):
    if file_path.suffix == ".ipynb":
        return NotaBeneNotebook.from_parent(
            parent,
            name=file_path.name,
            notebook_path=file_path,
        )
```

### `NotaBeneNotebook`

Reads the notebook JSON and yields a `NotaBeneCellTest` for each cell that has a `nota-bene.test` entry:

```python
class NotaBeneNotebook(pytest.Collector):
    def __init__(self, name, parent, notebook_path):
        super().__init__(name, parent)
        self.notebook_path = notebook_path

    def collect(self):
        notebook = json.loads(self.notebook_path.read_text())
        for cell in notebook["cells"]:
            nb_meta = cell.get("metadata", {}).get("nota-bene", {})
            test = nb_meta.get("test")
            if test:
                yield NotaBeneCellTest.from_parent(
                    self,
                    name=test["name"],
                    notebook_path=self.notebook_path,
                    cell_id=cell["id"],
                    test_source="".join(test["source"]),
                )
```

### `NotaBeneCellTest`

Represents a single cell's test. Defers kernel execution until the first subtest runs. Yields `NotaBeneSubtest` items from the cached results:

```python
class NotaBeneCellTest(pytest.Collector):
    def __init__(self, name, parent, notebook_path, cell_id, test_source):
        super().__init__(name, parent)
        self.notebook_path = notebook_path
        self.cell_id = cell_id
        self.test_source = test_source
        self._results = None

    def run_kernel(self):
        """Deferred kernel execution — called by the first subtest, cached for the rest."""
        if self._results is None:
            self._results = execute_cell_test(
                self.notebook_path,
                self.cell_id,
                self.test_source,
            )
        return self._results

    def collect(self):
        # Subtests are not known until the kernel runs, so we yield a placeholder
        # that triggers execution on first runtest(). The placeholder then dynamically
        # yields real subtests after execution.
        yield NotaBeneCellTestRunner.from_parent(self, name="[run]")
```

### `NotaBeneSubtest`

A single subtest result replayed as a pytest item:

```python
class NotaBeneSubtest(pytest.Item):
    def __init__(self, name, parent, result):
        super().__init__(name, parent)
        self.result = result

    def runtest(self):
        if not self.result["passed"]:
            raise NotaBeneTestFailure(self.result)

    def repr_failure(self, excinfo):
        result = excinfo.value.result
        lines = [f"Subtest failed: {self.name}"]
        if result.get("traceback"):
            lines.append("")
            lines.append(result["traceback"])
        if result.get("error"):
            lines.append(result["error"])
        return "\n".join(lines)

    def reportinfo(self):
        return self.path, None, f"{self.parent.name}::{self.name}"
```

## Kernel Lifecycle

Kernel execution is deferred — the kernel does not start at collection time. Collection is intended to be fast and side-effect-free. Instead, execution is triggered by the first subtest's `runtest()` call and the results are cached on `NotaBeneCellTest` for all subsequent subtests.

### Execution sequence

For each cell test, `execute_cell_test()` performs the following steps inside the kernel:

```
1. Start kernel (jupyter_client KernelManager)
2. Inject nota_bene library into kernel namespace
3. For each preceding cell (cumulative chain):
   a. Run that cell's fixtures (sorted by priority)
   b. Run that cell's patched source (or unpatched if no diff)
4. Run current cell's fixtures (sorted by priority)
5. Call nota_bene._runner.load_cell(patched_source) to inject the target cell's source
6. Run the test source
7. Call nota_bene._runner.get_test_results() to retrieve results as JSON
8. Send nota_bene._runner.run_teardowns()
9. Shut down the kernel
10. Return the results list
```

If the test source raises an uncaught exception (i.e. no `subtest()` calls caught it), the runner constructs an implicit single result using the test name and the exception details.

### Sketch of `execute_cell_test`

```python
def execute_cell_test(notebook_path, cell_id, test_source):
    notebook = json.loads(notebook_path.read_text())
    cells = notebook["cells"]
    target_idx = next(i for i, c in enumerate(cells) if c["id"] == cell_id)

    km = jupyter_client.KernelManager()
    km.start_kernel()
    kc = km.client()
    kc.start_channels()
    kc.wait_for_ready()

    try:
        # inject nota_bene
        execute(kc, "import nota_bene")

        # cumulative chain: all cells before target
        for cell in cells[:target_idx]:
            run_cell_fixtures(kc, cell)
            run_cell_source(kc, cell)

        # current cell: fixtures only, then load patched source for execute_cell()
        run_cell_fixtures(kc, cells[target_idx])
        patched_source = get_patched_source(cells[target_idx])
        execute(kc, f"nota_bene._runner.load_cell({json.dumps(patched_source)})")

        # run test source
        execute(kc, test_source)

        # collect results
        results_json = execute_and_capture(kc, "nota_bene._runner.get_test_results()")
        results = json.loads(results_json)

        # teardown
        execute(kc, "nota_bene._runner.run_teardowns()")

        return results

    finally:
        kc.stop_channels()
        km.shutdown_kernel()
```

## Result Mapping and Failure Reporting

After `execute_cell_test()` returns, `NotaBeneCellTest` has a list of result dicts. Each becomes a `NotaBeneSubtest`. If the list has one entry (either explicit or implicit), the subtest level is still present in the hierarchy — the test name and subtest name will be the same, and pytest collapses it to a single line in output.

### Failure output

When a subtest fails, `repr_failure` surfaces the kernel-side traceback rather than the pytest-side raise. A failing subtest looks like:

```
FAILED my_notebook.ipynb::validates price calculation::price=0.0 quantity=5

Subtest failed: price=0.0 quantity=5

  File "<test>", line 12
    assert (df["total"] == case["expected_total"]).all(), (
AssertionError: expected 0.0, got 5.0
```

Because all subtests are pre-collected from the kernel before any `runtest()` calls replay them, **one failing subtest does not prevent others from running or reporting**. All subtests always report.

## Requirements

**pytest-side environment** (where `pytest` runs):
- `pytest`
- `jupyter_client`
- `nota-bene`

**Kernel-side environment** (determined by the notebook's kernel spec):
- `nota_bene`
- The notebook's own dependencies (pandas, etc.)

These can be the same Python environment or different ones. The pytest process communicates with the kernel over ZMQ — the kernel runs in whatever environment its kernel spec points to.
