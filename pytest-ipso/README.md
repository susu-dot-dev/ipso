# pytest-ipso

A pytest plugin that discovers and runs [ipso](https://pypi.org/project/ipso/) notebook cell tests.

## How it works

ipso stores test metadata (fixtures, patches, assertions) directly on Jupyter notebook cells. This plugin collects `.ipynb` files, surfaces each cell that has an ipso test as a `notebook.ipynb::test name` item, delegates execution to the `ipso` CLI, and maps the results back into pytest's reporting.

## Installation

```sh
pip install pytest-ipso
```

`ipso` is installed automatically as a dependency.

## Usage

```sh
pytest path/to/notebook.ipynb   # run cell tests in one notebook
pytest                          # discover .ipynb files alongside .py tests
pytest --ipso-timeout=120       # override per-cell execution timeout (default: 60s)
```

Each cell test runs in its own isolated kernel, so it never affects your live notebook state.

## Setting up tests

Tests are stored as cell metadata, not in the notebook source. Use the `ipso` CLI or an AI assistant with the ipso MCP server to create and manage them. See the [ipso docs](https://pypi.org/project/ipso/) for details.
