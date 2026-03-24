# Nota-bene - Closing the loop for AI-powered jupyter notebooks

AI works best with a feedback loop - make a change, verify the syntax, run the tests, and iterate.

Notebooks don't really work that way. There are no functions, everything is a global variable, and there's no place to put tests, even if you wanted to.

Nota-bene connects these worlds by using AI to mantain shadow metadata within a notebook to hardcode, setup, and otherwise mock any setup needed for cells to run, along with tests to ensure the cells behave properly.

This works continuously behind the scenes as you & your agents work on a notebook together. The Nota-bene LSP server notifies AI when the metadata has gone stale. Next, the MCP server guides the AI to make the appropriate changes, to create and ensure the cells work correctly. Finally, Nota-bene will run the tests in an independent kernel, to ensure that the code works correctly, without messing with your actual work.

# Installing Nota-bene
Nota-bene needs to be installed in the python environment that your kernel is running in. `pip install nota-bene` will work, and `pip install pytest-nota-bene` is also needed if you want to be able to run `pytest` and have it automatically detect and run any nota-bene tests (optional)

Next, for the AI loop to work properly, you need to enable the LSP and MCP servers for your AI application. You can either point your AI to the python environment you created, or you can use something like `uvx` or `pipx` to create a global instance. For example:

```json
{
  "$schema": "https://opencode.ai/config.json",
  "mcp": {
    "nota-bene": {
      "type": "local",
      "command": ["uvx", "nota-bene", "mcp"]
    }
  },
  "lsp": {
    "nota-bene": {
      "command": ["uvx", "nota-bene", "lsp"],
      "extensions": [".ipynb"]
    }
  }
}
```

# Using Nota-bene
Just use your notebook how you normally would! As you make changes, the LSP will inform the AI to keep the metadata in sync, and it will use the MCP tool to investigate and fix any errors. Hopefully, this will allow the AI to catch any mistakes in the code it generates, or prompt further discussion as to the desired behavior (especially when changing existing logic)

To see the tests yourself, you can just run `pytest "path/to/notebook"`, or you can use the nota-bene CLI to investigate, run, and edit the tests.

# The CLI

The CLI lets you inspect, edit, and run the metadata that nota-bene attaches to each cell (fixtures, patches, and tests) without needing an AI in the loop.

## Checking notebook health

```sh
# Quick check: are any cells out of date?
nota-bene status notebook.ipynb

# See everything nota-bene knows about your cells
nota-bene view notebook.ipynb

# Narrow it down — only cells that have tests
nota-bene view notebook.ipynb --filter "test:not null"

# Only show the fields you care about
nota-bene view notebook.ipynb --fields source,status
```

`status` exits non-zero when something needs attention, so it works well in CI or pre-commit hooks.

## Running tests

```sh
# Run all cell tests
nota-bene test notebook.ipynb

# Run tests for specific cells
nota-bene test notebook.ipynb --filter "cell:abc123"

# Use a specific python environment
nota-bene test notebook.ipynb --python .venv/bin/python
```

Each cell's test runs in its own isolated kernel, so it never interferes with your live notebook state.

## Editing metadata by hand

The easiest way to edit fixtures and tests directly is with the editor notebook:

```sh
# Open a side-by-side editor notebook
nota-bene edit notebook.ipynb

# ... make changes in notebook.nota-bene.ipynb ...

# Apply your edits back to the source notebook
nota-bene edit --continue notebook.ipynb
```

The editor notebook lays out each cell's fixtures, patched source, and tests as editable cells you can work with in Jupyter. When you're done, `--continue` folds everything back into the source notebook's metadata.

If you prefer working with JSON directly, `update` lets you apply changes programmatically:

```sh
# Set a fixture and test on a cell
nota-bene update notebook.ipynb --data '{
  "cell_id": "abc123",
  "fixtures": {"setup_db": {"description": "mock database", "source": "db = MockDB()"}},
  "test": {"name": "test_query", "source": "assert query(db) == expected"}
}'

# scaffold well-formed JSON fragments to avoid typos
nota-bene scaffold fixture --name setup_db --source "db = MockDB()"
nota-bene scaffold test --name test_query --source "assert query(db) == expected"
```

## Accepting changes

When you've reviewed a cell and are satisfied that its metadata is correct despite the diagnostics, you can accept it to clear the warnings:

```sh
# Accept all cells
nota-bene accept notebook.ipynb --all

# Accept only specific cells
nota-bene accept notebook.ipynb --filter "cell:abc123"
```

## Filtering

Most commands support `--filter` to target specific cells. Filters can match on cell ID, index, presence of metadata, validity status, and diagnostic type or severity. Multiple filters are AND-ed together; comma-separated values within a filter are OR-ed. Run `nota-bene docs filters` for the full reference.
