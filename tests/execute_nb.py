#!/usr/bin/env python3
"""Execute a notebook and write the result (with outputs) to a separate file.

Usage: execute_nb.py <input.ipynb> <output.ipynb>

The input file is never modified.
"""

import sys
import nbformat
from nbclient import NotebookClient

input_path, output_path = sys.argv[1], sys.argv[2]

nb = nbformat.read(input_path, as_version=4)
NotebookClient(nb, timeout=60).execute()
nbformat.write(nb, output_path)
