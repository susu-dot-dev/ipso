"""nota_bene — in-kernel library for notebook cell testing.

Public API for test authors and fixture source code:

    nota_bene.execute_cell()            # run the loaded cell source
    nota_bene.subtest(name)             # context manager for named subtests
    nota_bene.register_teardown(fn)     # register a cleanup callback

Runner-facing API (called by pytest-nota-bene, not test code):

    nota_bene._runner.load_cell(source)         # inject patched source
    nota_bene._runner.get_test_results()        # retrieve results as JSON
    nota_bene._runner.run_teardowns()           # drain teardown stack
"""

from __future__ import annotations

import traceback as _traceback
from collections.abc import Callable, Generator
from contextlib import contextmanager

import nota_bene._state as _state
from nota_bene import _runner
from nota_bene.__about__ import __version__
from nota_bene._state import SubtestResult

__all__ = [
    "__version__",
    "_runner",
    "SubtestResult",
    "execute_cell",
    "register_teardown",
    "subtest",
]


def execute_cell() -> None:
    """Run the patched cell source in the kernel's global namespace.

    The source must have been loaded by _runner.load_cell() before this is
    called. Raises RuntimeError if no source has been loaded.

    The source is executed via exec() in the caller's global namespace so
    that any variables assigned by the cell (e.g. ``df``) are visible to
    subsequent test code. If the cell raises an exception it propagates
    to the caller — it is not swallowed.

    Can be called multiple times within a single test; each call
    re-executes the same source.
    """
    if _state.cell_source is None:
        raise RuntimeError(
            "nota_bene.execute_cell() called before nota_bene._runner.load_cell(). "
            "The runner must inject the cell source before the test runs."
        )
    import inspect as _inspect

    frame = _inspect.currentframe()
    caller_globals = frame.f_back.f_globals if frame and frame.f_back else {}
    exec(_state.cell_source, caller_globals)  # noqa: S102


@contextmanager
def subtest(name: str) -> Generator[None, None, None]:
    """Context manager that records a named subtest result.

    On exit without exception appends a passing result to _state.test_results.
    On exit with exception appends a failing result (with error message and
    traceback) and suppresses the exception so subsequent subtests can run.

    Args:
        name: Human-readable name for this subtest, used in pytest reporting.
    """
    try:
        yield
    except Exception as exc:
        tb = _traceback.format_exc()
        _state.test_results.append(
            SubtestResult(
                name=name,
                passed=False,
                error=str(exc),
                traceback=tb,
            )
        )
    else:
        _state.test_results.append(
            SubtestResult(
                name=name,
                passed=True,
                error=None,
                traceback=None,
            )
        )


def register_teardown(callback: Callable[[], None]) -> None:
    """Push a cleanup callback onto the teardown stack.

    Callbacks are invoked by _runner.run_teardowns() in LIFO order after
    the test completes. Typically called from fixture source, not test code.

    Args:
        callback: A callable that takes no arguments.
    """
    _state.teardown_stack.append(callback)
