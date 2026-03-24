"""Execute a notebook from stdin via nbclient, write the result to stdout.

Invoked by the ipso Rust CLI as: python -m ipso._executor [timeout]
"""

import sys

import nbformat
from nbclient import NotebookClient

nb = nbformat.reads(sys.stdin.read(), as_version=4)  # type: ignore[no-untyped-call]
timeout = int(sys.argv[1]) if len(sys.argv) > 1 else 60

try:
    NotebookClient(nb, timeout=timeout, allow_errors=True).execute()
except Exception as e:
    print(f"__NB_EXEC_ERROR__{e}", file=sys.stderr)

nbformat.write(nb, sys.stdout)  # type: ignore[no-untyped-call]
