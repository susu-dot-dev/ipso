"""Runner-facing API for nota_bene.

These functions are called by the pytest runner (pytest-nota-bene) via
execute_request messages sent to the kernel over ZMQ. They are not
intended for use in test code or fixture source.

Usage from the runner:
    nota_bene._runner.load_cell(patched_source)
    nota_bene._runner.get_test_results()   # returns JSON string
    nota_bene._runner.run_teardowns()
"""

import json as _json
import traceback as _traceback

import nota_bene._state as _state


def load_cell(source: str) -> None:
    """Inject the already-patched cell source string into the kernel.

    Called by the runner exactly once per test, for the target cell only.
    The runner is responsible for joining the cell's source array and
    applying any unified diff before calling this. This function stores
    the string as-is with no validation or transformation.

    Args:
        source: The patched cell source string to be executed by execute_cell().
    """
    _state.cell_source = source


def get_test_results() -> str:
    """Return the accumulated subtest results as a JSON string.

    Called by the runner after the test source finishes executing to read
    results out of the kernel. The runner deserializes the returned string
    with json.loads().

    Returns:
        A JSON string representing a list of subtest result dicts.
    """
    return _json.dumps(_state.test_results)


def run_teardowns() -> None:
    """Drain the teardown stack in LIFO order.

    Called by the runner after the test source finishes executing, before
    kernel shutdown. All registered callbacks are called regardless of
    individual failures — if a callback raises, the error is printed and
    the remaining callbacks continue to run.
    """
    while _state.teardown_stack:
        callback = _state.teardown_stack.pop()
        try:
            callback()
        except Exception:
            _traceback.print_exc()
