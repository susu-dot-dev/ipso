Important: This document contains the original inspiration for nota-bene. It has been superceeded by the other documents in the codebase. Where there are discrepancies in the implementation, the content in this file should be considered incorrect. This file will likely get deleted at some point.

# Problem
How do you test a notebook?

Python notebooks and other REPLs are easy to prototype with, and hard to test. They're hard to test because they lack abstractions and are full of side effects. A production codebase would have a function, care taken with the inputs & outputs to make it easy to pass in the data the function needs, while minimizing side-effects. Then, unit tests can be written in a separate file, calling these functions directly, and working around limitations (such as using mocks for inputs or side effects). Lastly, tests need to be performant, and ideally parallelizable. We can't load a 3gb file for every test, even though the notebook itself might be parsing a large dataset.

So, to summarize, we have three problems to solve with notebook testing:
1. Where do the tests live?
2. How do we set up an isolated environment where all the prerequisites are called, such that notebook code, which uses globals all over the place, runs successfully?
3. How can we make the tests performant, even though the notebook might be crunching through lots of data?

# AI is the problem and the solution
Notebooks have not historically needed to solve this problem, because notebooks are ad-hoc tests. The REPL loop is essentially a test. Print what the cell does, a human decides if it's good or not, changes the code, and continues. The flaws in this approach, such as polluting the global state with each attempt, having to validate the code in the same environment that you're running it, and only being able to test one thing at a time aren't huge problems because the human is always in the loop. The developer can reason when they need to restart the kernel because they messed up the global state. Tests don't need to run in parallel, because the author is only changing one thing at a time. We don't need to re-run the dataframe loading each time because there's only one environment, and so on.

These flaws break the moment AI is in the loop. AI can do many things at once. It needs a tight feedback loop to improve the code outputs. It may want to run debugging and introspection code to figure out what is happening in some temporary state.

However, AI can also be the solution to this problem. Writing code is cheap. Keeping code in sync with other code is also not the headache it used to be. So we can put AI to work behind the scenes: keep the notebook, and have AI generate fixtures, patches, and the tests themselves to verify behavior.

# Basic concept
Jupyter notebooks let us store [arbitrary metadata](https://nbformat.readthedocs.io/en/latest/format_description.html) at both the notebook level, as well as per-cell. We'll use that to store the following per-cell data:
- nota-bene.fixtures: Test fixtures which create variables, functions, or mocks needed to enable tests to run. The dict key is the fixture name, and each fixture has:
    - description: Information about the fixture, what it does, and its relation to the notebook
    - update_when: Instructions to the AI about when the fixture needs to be updated and what is required to keep it in sync with the production code
    - depends: other fixtures that must be run before this fixture
    - source: the fixture Python content
- nota-bene.diff: diff-style patch, containing any changes needed to adjust the cell source to use the fixtures or otherwise be aware of the test environment
- nota-bene.test: tests which use assertions to validate that the cell performed correctly (e.g. checking state or cell outputs). The value is a dict with two keys:
    - depends: Other fixture the test depends on to run
    - assertions: Python code to validate the output is correct. 
- nota-bene.sha1: Sha of the cell content when the nota-bene fields were last validated to work properly

Let's see this in a short example. Given this cell content:

```py
import pandas as pd

df = pd.read_csv('your_file.csv')
df.head()
```

and let's assume the CSV is quite large, so we don't actually want to read all of it. There are a couple of ways we could decide to test this cell. Perhaps, we just want to read the first 10 lines and validate that the csv gets loaded. That could turn into this fixture:

```py
import tempfile
import shutil
from contextlib import contextmanager

@contextmanager
def test_csv():
    src = 'your_file.csv'
    with tempfile.NamedTemporaryFile(mode='w+', suffix='.csv') as tmp:
        with open(src, 'r') as orig:
            for i, line in enumerate(orig):
                if i < 10:
                    tmp.write(line)
                else:
                    break
        tmp.flush()
        global csv_name
        csv_name = tmp.name
        yield tmp.name
```

and then the diff could look like this:

```diff
--- a/cell
+++ b/cell
@@ -1,5 +1,5 @@
 import pandas as pd
 
-df = pd.read_csv('your_file.csv')
+df = pd.read_csv(csv_name)
 df.head()
```

with the following test code:

```json
{
  "depends": ["test_csv"],
  "assertions": "assert 'df' in dir() and df is not None\nassert isinstance(df, pd.DataFrame)\nassert len(df) > 0\nout = _cell_outputs[-1]\nassert out.get('output_type') == 'execute_result'\nassert 'text/plain' in out.get('data', {})\n"
}
```

# Executing the tests
Now, given the known python environment, we can create a new jupyter kernel, and then construct the test by:
1. Following the DAG of depends to load the fixtures, and nested fixtures
2. Apply the diff to the source cell, to get the modified content
3. Run the modified cell
4. Run the assertions and ensure they pass

Here's the pseudocode to launch the kernel (`test_setup` is the concatenated fixture source for this test's dependencies):
```py
from IPython.core.interactiveshell import InteractiveShell
from IPython.utils.capture import capture_output
shell = InteractiveShell.instance()
shell.user_ns["_cell_cwd"] = str(cwd)

if test_meta.test_setup.strip():
    res = shell.run_cell(test_meta.test_setup)
    if not res.success:
        res.raise_error()

with capture_output() as cap:
    res = shell.run_cell(cell_source)

shell.user_ns["_cell_result"] = res.result
shell.user_ns["_cell_stdout"] = cap.stdout
shell.user_ns["_cell_stderr"] = cap.stderr
shell.user_ns["_cell_outputs"] = list(cap.outputs) if cap.outputs else []
shell.user_ns["_cell_success"] = res.success
```

# Using LSPs to provide instant feedback to AI frameworks
Agent frameworks, such as opencode, use LSP integrations to provide realtime feedback to AI. This is how AI knows when it generates code that doesn't compile, and then it fixes it without any user input.

As long as it's fast enough, we can use this to provide in-the-loop feedback to the AI about the state of the nota-bene tests. The feedback can directly provide the result, with a suggestion to use the MCP tools to modify the tests as needed, or to validate that the tests are still correct after editing the cells.

The LSP integration isn't strictly required and has drawbacks. An alternative is to add information to the agent's context so it is encouraged to call the correct MCP tools after modifying a cell. That has other tradeoffs; we can experiment to find the best approach.

# Using MCP tools to guide AI development of nota-bene tests
We can create MCP tools to navigate the fixtures, the tests, and the assertions as necessary. Some examples:

- materialize: combine all the setup and test code into a single python file (or string rather), so that AI can easily see the entire test - including setup and patching - to help the AI guide the setup

- create_fixture: Create a fixture for a given cell
- create_test: Adds or updates the cell tests
- keep_updated: Return a list of all the nota-bene cells which are potentially out of date, as well as descriptions about when the files should be updated
- run_tests: Run all or some of the cells to validate they work

# Putting it all together
Using some combination of llms.txt, MCP tools, skills, or other context munging, the Agent framework is aware of nota-bene. When the agent updates the notebook cells, it knows that it also needs to update the nota-bene tests as well. So, it uses the MCP tools to create the fixtures and tests. Later on, as more edits are made, the tests are run in isolated per-cell kernels (perhaps using LSP, or hooking into the cell execution of the primary notebook, or manually via MCP) and provide feedback in the loop about the test execution

# Bonus: Playgrounds
The ability to recreate an environment, quickly, is really powerful for developing new features. We can create many kernels, run the fixtures in order to recreate the global state from the preceding cells. However, instead of running the cell & tests (since there isn't cell code yet), we can just let the AI play around, write code, and execute to make sure things work. This is the agentic REPL: parallel, ephemeral, and able to prove the basic case without running a full pipeline.
