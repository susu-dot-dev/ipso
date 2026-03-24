"""Integration tests for the pytest-ipso plugin using pytester."""

from __future__ import annotations

from pathlib import Path

import pytest

FIXTURES_DIR = Path(__file__).parent.parent.parent / "tests" / "fixtures"


# ---------------------------------------------------------------------------
# Helper
# ---------------------------------------------------------------------------


def copy_fixture(pytester: pytest.Pytester, name: str) -> Path:
    src = FIXTURES_DIR / name
    dest = pytester.path / name
    dest.write_text(src.read_text(encoding="utf-8"), encoding="utf-8")
    return dest


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


def test_passing_test(pytester: pytest.Pytester) -> None:
    """A passing test reports PASSED and exits 0."""
    copy_fixture(pytester, "test-pass.ipynb")
    result = pytester.runpytest("-v")
    result.stdout.fnmatch_lines(["*total is 60*PASSED*"])
    assert result.ret == 0


# ---------------------------------------------------------------------------
# 3. Assertion failure
# ---------------------------------------------------------------------------


def test_assertion_failure(pytester: pytest.Pytester) -> None:
    """A failing subtest reports FAILED and exits 1."""
    copy_fixture(pytester, "test-fail-assertion.ipynb")
    result = pytester.runpytest("-v")
    result.stdout.fnmatch_lines(["*test-fail-assertion.ipynb::x equals 1*FAILED*"])
    assert result.ret == 1


# ---------------------------------------------------------------------------
# 4. Multiple subtests
# ---------------------------------------------------------------------------


def test_multiple_subtests_appear_in_output(pytester: pytest.Pytester) -> None:
    """All subtests appear in output regardless of individual pass/fail."""
    copy_fixture(pytester, "test-subtests.ipynb")
    result = pytester.runpytest("-v")
    result.stdout.fnmatch_lines(["*multiplication result*FAILED*"])
    # The failure report should mention both subtests
    result.stdout.fnmatch_lines(["*correct result*"])
    result.stdout.fnmatch_lines(["*wrong assertion*"])
    assert result.ret == 1


# ---------------------------------------------------------------------------
# 5. Fixture error
# ---------------------------------------------------------------------------


def test_fixture_error(pytester: pytest.Pytester) -> None:
    """A fixture that raises is reported as an infrastructure error."""
    copy_fixture(pytester, "test-fixture-error.ipynb")
    result = pytester.runpytest("-v")
    result.stdout.fnmatch_lines(["*should not reach test*FAILED*"])
    # Should mention fixture phase
    result.stdout.fnmatch_lines(["*fixture*"])
    assert result.ret != 0


# ---------------------------------------------------------------------------
# 6. Multi-cell chain
# ---------------------------------------------------------------------------


def test_multi_cell_chain(pytester: pytest.Pytester) -> None:
    """A test cell that depends on preceding cells' state works correctly."""
    copy_fixture(pytester, "test-multi-cell.ipynb")
    result = pytester.runpytest("-v")
    result.stdout.fnmatch_lines(["*a equals 10*PASSED*"])
    result.stdout.fnmatch_lines(["*b equals 20*PASSED*"])
    assert result.ret == 0


# ---------------------------------------------------------------------------
# 7. Diff
# ---------------------------------------------------------------------------


def test_diff_applied(pytester: pytest.Pytester) -> None:
    """A cell with ipso.diff applies the patch before execution."""
    copy_fixture(pytester, "test-with-diff.ipynb")
    result = pytester.runpytest("-v")
    result.stdout.fnmatch_lines(["*diff is applied*PASSED*"])
    assert result.ret == 0


# ---------------------------------------------------------------------------
# 8. No interference (execution)
# ---------------------------------------------------------------------------


def test_no_interference_execution(pytester: pytest.Pytester) -> None:
    """Regular .py test files still pass when the plugin is active."""
    pytester.makepyfile(
        test_plain="""
def test_regular():
    assert 1 + 1 == 2
"""
    )
    result = pytester.runpytest("-v")
    result.stdout.fnmatch_lines(["*test_plain.py::test_regular*PASSED*"])
    assert result.ret == 0
