"""Integration tests for the pytest-nota-bene plugin using pytester."""

from __future__ import annotations

import shutil
from pathlib import Path

import pytest

FIXTURES_DIR = Path(__file__).parent.parent.parent / "tests" / "fixtures"

# The nota-bene binary: prefer the local build, fall back to PATH.
_LOCAL_BINARY = Path(__file__).parent.parent.parent / "target" / "debug" / "nota-bene"


def _nb_binary() -> str | None:
    if _LOCAL_BINARY.exists():
        return str(_LOCAL_BINARY)
    found = shutil.which("nota-bene")
    return found


NB_BINARY = _nb_binary()

needs_nb = pytest.mark.skipif(NB_BINARY is None, reason="nota-bene binary not found")


# ---------------------------------------------------------------------------
# Helper
# ---------------------------------------------------------------------------


def copy_fixture(pytester: pytest.Pytester, name: str) -> Path:
    src = FIXTURES_DIR / name
    dest = pytester.path / name
    dest.write_text(src.read_text(encoding="utf-8"), encoding="utf-8")
    return dest


def nb_args() -> list[str]:
    """Common args to pass nota-bene binary location."""
    assert NB_BINARY is not None
    return [f"--nb-binary={NB_BINARY}"]


# ---------------------------------------------------------------------------
# 1. Collection
# ---------------------------------------------------------------------------


def test_collection_discovers_test_cell(pytester: pytest.Pytester) -> None:
    """A notebook with one test cell is collected as notebook.ipynb::test name."""
    copy_fixture(pytester, "test-pass.ipynb")
    result = pytester.runpytest("--collect-only", "-q")
    result.stdout.fnmatch_lines(["*test-pass.ipynb::total is 60*"])


def test_collection_multi_cell(pytester: pytest.Pytester) -> None:
    """A notebook with two test cells yields two items."""
    copy_fixture(pytester, "test-multi-cell.ipynb")
    result = pytester.runpytest("--collect-only", "-q")
    result.stdout.fnmatch_lines(["*test-multi-cell.ipynb::a equals 10*"])
    result.stdout.fnmatch_lines(["*test-multi-cell.ipynb::b equals 20*"])


def test_no_interference_with_py_files(pytester: pytest.Pytester) -> None:
    """Regular .py test files are unaffected by the plugin."""
    pytester.makepyfile(
        test_plain="""
def test_regular():
    assert 1 + 1 == 2
"""
    )
    result = pytester.runpytest("--collect-only", "-q")
    result.stdout.fnmatch_lines(["*test_plain.py::test_regular*"])


# ---------------------------------------------------------------------------
# 2. Pass
# ---------------------------------------------------------------------------


@needs_nb
def test_passing_test(pytester: pytest.Pytester) -> None:
    """A passing test reports PASSED and exits 0."""
    copy_fixture(pytester, "test-pass.ipynb")
    result = pytester.runpytest("-v", *nb_args())
    result.stdout.fnmatch_lines(["*total is 60*PASSED*"])
    assert result.ret == 0


# ---------------------------------------------------------------------------
# 3. Assertion failure
# ---------------------------------------------------------------------------


@needs_nb
def test_assertion_failure(pytester: pytest.Pytester) -> None:
    """A failing subtest reports FAILED and exits 1."""
    copy_fixture(pytester, "test-fail-assertion.ipynb")
    result = pytester.runpytest("-v", *nb_args())
    result.stdout.fnmatch_lines(["*test-fail-assertion.ipynb::x equals 1*FAILED*"])
    assert result.ret == 1


# ---------------------------------------------------------------------------
# 4. Multiple subtests
# ---------------------------------------------------------------------------


@needs_nb
def test_multiple_subtests_appear_in_output(pytester: pytest.Pytester) -> None:
    """All subtests appear in output regardless of individual pass/fail."""
    copy_fixture(pytester, "test-subtests.ipynb")
    result = pytester.runpytest("-v", *nb_args())
    result.stdout.fnmatch_lines(["*multiplication result*FAILED*"])
    # The failure report should mention both subtests
    result.stdout.fnmatch_lines(["*correct result*"])
    result.stdout.fnmatch_lines(["*wrong assertion*"])
    assert result.ret == 1


# ---------------------------------------------------------------------------
# 5. Fixture error
# ---------------------------------------------------------------------------


@needs_nb
def test_fixture_error(pytester: pytest.Pytester) -> None:
    """A fixture that raises is reported as an infrastructure error."""
    copy_fixture(pytester, "test-fixture-error.ipynb")
    result = pytester.runpytest("-v", *nb_args())
    result.stdout.fnmatch_lines(["*should not reach test*FAILED*"])
    # Should mention fixture phase
    result.stdout.fnmatch_lines(["*fixture*"])
    assert result.ret != 0


# ---------------------------------------------------------------------------
# 6. Multi-cell chain
# ---------------------------------------------------------------------------


@needs_nb
def test_multi_cell_chain(pytester: pytest.Pytester) -> None:
    """A test cell that depends on preceding cells' state works correctly."""
    copy_fixture(pytester, "test-multi-cell.ipynb")
    result = pytester.runpytest("-v", *nb_args())
    result.stdout.fnmatch_lines(["*a equals 10*PASSED*"])
    result.stdout.fnmatch_lines(["*b equals 20*PASSED*"])
    assert result.ret == 0


# ---------------------------------------------------------------------------
# 7. Diff
# ---------------------------------------------------------------------------


@needs_nb
def test_diff_applied(pytester: pytest.Pytester) -> None:
    """A cell with nota-bene.diff applies the patch before execution."""
    copy_fixture(pytester, "test-with-diff.ipynb")
    result = pytester.runpytest("-v", *nb_args())
    result.stdout.fnmatch_lines(["*diff is applied*PASSED*"])
    assert result.ret == 0


# ---------------------------------------------------------------------------
# 8. No interference (execution)
# ---------------------------------------------------------------------------


@needs_nb
def test_no_interference_execution(pytester: pytest.Pytester) -> None:
    """Regular .py test files still pass when the plugin is active."""
    pytester.makepyfile(
        test_plain="""
def test_regular():
    assert 1 + 1 == 2
"""
    )
    result = pytester.runpytest("-v", *nb_args())
    result.stdout.fnmatch_lines(["*test_plain.py::test_regular*PASSED*"])
    assert result.ret == 0


# ---------------------------------------------------------------------------
# 9. Binary not found
# ---------------------------------------------------------------------------


def test_binary_not_found(pytester: pytest.Pytester) -> None:
    """Graceful error when nota-bene binary is not on PATH."""
    copy_fixture(pytester, "test-pass.ipynb")
    result = pytester.runpytest("-v", "--nb-binary=nota-bene-nonexistent-xyz")
    result.stdout.fnmatch_lines(["*FAILED*"])
    result.stdout.fnmatch_lines(["*nota-bene-nonexistent-xyz*"])
    assert result.ret != 0
