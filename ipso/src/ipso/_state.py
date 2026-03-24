"""Shared mutable state for the ipso in-kernel library.

Both the public API (ipso.__init__) and the runner-facing API
(ipso._runner) read and write this module's variables. Neither
imports the other — both import _state directly to avoid circular imports.
"""

from collections.abc import Callable
from typing import TypedDict


class SubtestResult(TypedDict):
    """A single subtest result dict accumulated by subtest() and returned by
    _runner.get_test_results()."""

    name: str
    passed: bool
    error: str | None
    traceback: str | None


# The already-patched cell source string. Set by _runner.load_cell() before
# the test source runs. Read by execute_cell() when the test calls it.
cell_source: str | None = None

# Accumulated subtest result dicts for the current test. Appended to by
# subtest(). Read by the runner after the test source finishes via
# _runner.get_test_results().
test_results: list[SubtestResult] = []

# LIFO stack of teardown callbacks. Pushed by register_teardown().
# Drained by _runner.run_teardowns() after the test completes.
teardown_stack: list[Callable[[], None]] = []
