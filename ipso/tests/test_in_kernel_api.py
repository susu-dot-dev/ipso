"""Comprehensive tests for the ipso in-kernel API.

Coverage targets:
  - ipso._state      (module variables)
  - ipso._runner     (load_cell, get_test_results, run_teardowns)
  - ipso.__init__    (execute_cell, subtest, register_teardown)
"""

from __future__ import annotations

import json
from collections.abc import Generator
from typing import Any

import pytest

import ipso
import ipso._runner as _runner
import ipso._state as _state
from ipso._state import SubtestResult


# ---------------------------------------------------------------------------
# Helpers
# ---------------------------------------------------------------------------


def reset_state() -> None:
    """Reset all _state variables to their initial values between tests."""
    _state.cell_source = None
    _state.test_results = []
    _state.teardown_stack = []


@pytest.fixture(autouse=True)
def clean_state() -> Generator[None, None, None]:
    """Automatically reset shared state before every test."""
    reset_state()
    yield
    reset_state()


# ---------------------------------------------------------------------------
# _state — initial values
# ---------------------------------------------------------------------------


class TestState:
    def test_cell_source_initial_value(self):
        assert _state.cell_source is None

    def test_test_results_initial_value(self):
        assert _state.test_results == []

    def test_teardown_stack_initial_value(self):
        assert _state.teardown_stack == []


# ---------------------------------------------------------------------------
# _runner.load_cell
# ---------------------------------------------------------------------------


class TestLoadCell:
    def test_stores_source_in_state(self):
        _runner.load_cell("x = 1")
        assert _state.cell_source == "x = 1"

    def test_overwrites_previous_source(self):
        _runner.load_cell("x = 1")
        _runner.load_cell("x = 2")
        assert _state.cell_source == "x = 2"

    def test_stores_empty_string(self):
        _runner.load_cell("")
        assert _state.cell_source == ""

    def test_stores_multiline_source(self):
        src = "x = 1\ny = 2\nz = x + y\n"
        _runner.load_cell(src)
        assert _state.cell_source == src

    def test_does_not_touch_test_results(self):
        _state.test_results.append(SubtestResult(name="existing", passed=True, error=None, traceback=None))
        _runner.load_cell("x = 1")
        assert len(_state.test_results) == 1

    def test_does_not_touch_teardown_stack(self):
        _state.teardown_stack.append(lambda: None)
        _runner.load_cell("x = 1")
        assert len(_state.teardown_stack) == 1


# ---------------------------------------------------------------------------
# _runner.get_test_results
# ---------------------------------------------------------------------------


class TestGetTestResults:
    def test_returns_json_string(self):
        result = _runner.get_test_results()
        assert isinstance(result, str)
        json.loads(result)  # must be valid JSON

    def test_empty_when_no_subtests(self):
        result = json.loads(_runner.get_test_results())
        assert result == []

    def test_reflects_subtest_results(self):
        with ipso.subtest("case 1"):
            pass
        with ipso.subtest("case 2"):
            raise AssertionError("oops")

        results = json.loads(_runner.get_test_results())
        assert len(results) == 2
        assert results[0]["name"] == "case 1"
        assert results[0]["passed"] is True
        assert results[1]["name"] == "case 2"
        assert results[1]["passed"] is False

    def test_reflects_current_state_at_call_time(self):
        assert json.loads(_runner.get_test_results()) == []
        with ipso.subtest("added"):
            pass
        assert len(json.loads(_runner.get_test_results())) == 1

    def test_result_dict_keys_present(self):
        with ipso.subtest("check keys"):
            pass
        result = json.loads(_runner.get_test_results())[0]
        assert set(result.keys()) == {"name", "passed", "error", "traceback"}

    def test_passing_result_nulls(self):
        with ipso.subtest("passing"):
            pass
        result = json.loads(_runner.get_test_results())[0]
        assert result["error"] is None
        assert result["traceback"] is None

    def test_failing_result_has_error_and_traceback(self):
        with ipso.subtest("failing"):
            raise ValueError("something went wrong")
        result = json.loads(_runner.get_test_results())[0]
        assert result["error"] == "something went wrong"
        assert result["traceback"] is not None
        assert "ValueError" in result["traceback"]


# ---------------------------------------------------------------------------
# _runner.run_teardowns
# ---------------------------------------------------------------------------


class TestRunTeardowns:
    def test_calls_single_callback(self):
        called: list[int] = []
        _state.teardown_stack.append(lambda: called.append(1))
        _runner.run_teardowns()
        assert called == [1]

    def test_drains_stack_to_empty(self):
        _state.teardown_stack.append(lambda: None)
        _state.teardown_stack.append(lambda: None)
        _runner.run_teardowns()
        assert _state.teardown_stack == []

    def test_lifo_order(self):
        order: list[str] = []
        _state.teardown_stack.append(lambda: order.append("first"))
        _state.teardown_stack.append(lambda: order.append("second"))
        _state.teardown_stack.append(lambda: order.append("third"))
        _runner.run_teardowns()
        assert order == ["third", "second", "first"]

    def test_empty_stack_is_noop(self):
        _runner.run_teardowns()  # should not raise
        assert _state.teardown_stack == []

    def test_failing_callback_does_not_stop_others(self):
        order: list[str] = []

        def bad() -> None:
            raise ValueError("teardown failure")

        _state.teardown_stack.append(lambda: order.append("first"))
        _state.teardown_stack.append(bad)
        _state.teardown_stack.append(lambda: order.append("third"))

        _runner.run_teardowns()

        assert order == ["third", "first"]
        assert _state.teardown_stack == []

    def test_failing_callback_prints_traceback(self, capsys: pytest.CaptureFixture[str]) -> None:
        def bad() -> None:
            raise RuntimeError("boom")

        _state.teardown_stack.append(bad)
        _runner.run_teardowns()

        captured = capsys.readouterr()
        assert "RuntimeError" in captured.err
        assert "boom" in captured.err

    def test_multiple_failing_callbacks_all_run(self):
        calls: list[str] = []

        def bad1() -> None:
            raise ValueError("err1")

        def bad2() -> None:
            raise ValueError("err2")

        _state.teardown_stack.append(lambda: calls.append("ok"))
        _state.teardown_stack.append(bad1)
        _state.teardown_stack.append(bad2)

        _runner.run_teardowns()

        assert calls == ["ok"]
        assert _state.teardown_stack == []


# ---------------------------------------------------------------------------
# ipso.execute_cell
# ---------------------------------------------------------------------------


class TestExecuteCell:
    def test_raises_if_no_source_loaded(self):
        with pytest.raises(RuntimeError, match="load_cell"):
            ipso.execute_cell()

    def test_executes_source_in_caller_globals(self):
        _runner.load_cell("_test_var = 42")
        ipso.execute_cell()
        import sys

        caller_globals = sys._getframe(0).f_globals
        assert caller_globals.get("_test_var") == 42
        del caller_globals["_test_var"]

    def test_cell_side_effects_visible_after_call(self):
        _runner.load_cell("_nb_result = 1 + 1")
        ipso.execute_cell()
        import sys

        g = sys._getframe(0).f_globals
        assert g["_nb_result"] == 2
        del g["_nb_result"]

    def test_propagates_exception_from_cell(self):
        _runner.load_cell("raise ValueError('cell error')")
        with pytest.raises(ValueError, match="cell error"):
            ipso.execute_cell()

    def test_propagates_assertion_error(self):
        _runner.load_cell("assert False, 'bad cell'")
        with pytest.raises(AssertionError, match="bad cell"):
            ipso.execute_cell()

    def test_can_be_called_multiple_times(self):
        _runner.load_cell("_counter = globals().get('_counter', 0) + 1")
        ipso.execute_cell()
        ipso.execute_cell()
        ipso.execute_cell()
        import sys

        g = sys._getframe(0).f_globals
        assert g["_counter"] == 3
        del g["_counter"]

    def test_executes_empty_source(self):
        _runner.load_cell("")
        ipso.execute_cell()  # should not raise

    def test_executes_multiline_source(self):
        _runner.load_cell("_a = 3\n_b = 4\n_hyp = (_a**2 + _b**2) ** 0.5\n")
        ipso.execute_cell()
        import sys

        g = sys._getframe(0).f_globals
        assert g["_hyp"] == 5.0
        del g["_a"], g["_b"], g["_hyp"]


# ---------------------------------------------------------------------------
# ipso.subtest
# ---------------------------------------------------------------------------


class TestSubtest:
    def test_passing_subtest_appends_passing_result(self):
        with ipso.subtest("my case"):
            pass
        assert len(_state.test_results) == 1
        result = _state.test_results[0]
        assert result["name"] == "my case"
        assert result["passed"] is True
        assert result["error"] is None
        assert result["traceback"] is None

    def test_failing_subtest_appends_failing_result(self):
        with ipso.subtest("bad case"):
            raise AssertionError("things are wrong")
        assert len(_state.test_results) == 1
        result = _state.test_results[0]
        assert result["name"] == "bad case"
        assert result["passed"] is False
        assert result["error"] is not None
        assert "things are wrong" in result["error"]
        assert result["traceback"] is not None

    def test_failing_subtest_suppresses_exception(self):
        with ipso.subtest("suppressed"):
            raise RuntimeError("should be suppressed")
        # Execution reaches here
        assert _state.test_results[0]["passed"] is False

    def test_multiple_subtests_all_recorded(self):
        with ipso.subtest("case 1"):
            pass
        with ipso.subtest("case 2"):
            raise ValueError("oops")
        with ipso.subtest("case 3"):
            pass

        assert len(_state.test_results) == 3
        assert _state.test_results[0]["name"] == "case 1"
        assert _state.test_results[0]["passed"] is True
        assert _state.test_results[1]["name"] == "case 2"
        assert _state.test_results[1]["passed"] is False
        assert _state.test_results[2]["name"] == "case 3"
        assert _state.test_results[2]["passed"] is True

    def test_failing_subtest_does_not_stop_subsequent_subtests(self):
        executed: list[str] = []
        with ipso.subtest("first"):
            raise ValueError("fail")
        executed.append("after first")
        with ipso.subtest("second"):
            pass
        executed.append("after second")

        assert executed == ["after first", "after second"]
        assert len(_state.test_results) == 2

    def test_traceback_contains_source_location(self):
        with ipso.subtest("tb check"):
            raise AssertionError("marker error")
        tb = _state.test_results[0]["traceback"]
        assert tb is not None
        assert "AssertionError" in tb
        assert "marker error" in tb

    def test_error_string_matches_str_of_exception(self):
        exc_msg = "precise error message 12345"
        with ipso.subtest("err str"):
            raise ValueError(exc_msg)
        assert _state.test_results[0]["error"] == exc_msg

    def test_data_driven_pattern(self):
        """Mirrors the data-driven test pattern from the spec."""
        test_cases = [
            {"input": 1, "expected": 2},
            {"input": 2, "expected": 99},  # will fail
            {"input": 3, "expected": 6},
        ]

        def double(x: int) -> int:
            return x * 2

        for case in test_cases:
            with ipso.subtest(f"input={case['input']}"):
                assert double(case["input"]) == case["expected"]

        assert _state.test_results[0]["passed"] is True
        assert _state.test_results[1]["passed"] is False
        assert _state.test_results[2]["passed"] is True

    def test_subtest_name_preserved_exactly(self):
        name = "price=10.0 quantity=2 / edge case"
        with ipso.subtest(name):
            pass
        assert _state.test_results[0]["name"] == name


# ---------------------------------------------------------------------------
# ipso.register_teardown
# ---------------------------------------------------------------------------


class TestRegisterTeardown:
    def test_pushes_callback_onto_stack(self):
        fn = lambda: None  # noqa: E731
        ipso.register_teardown(fn)
        assert _state.teardown_stack == [fn]

    def test_multiple_registrations_in_order(self):
        fn1 = lambda: None  # noqa: E731
        fn2 = lambda: None  # noqa: E731
        fn3 = lambda: None  # noqa: E731
        ipso.register_teardown(fn1)
        ipso.register_teardown(fn2)
        ipso.register_teardown(fn3)
        assert _state.teardown_stack == [fn1, fn2, fn3]

    def test_registered_callbacks_run_lifo_via_runner(self):
        order: list[str] = []
        ipso.register_teardown(lambda: order.append("first"))
        ipso.register_teardown(lambda: order.append("second"))
        _runner.run_teardowns()
        assert order == ["second", "first"]

    def test_callback_registered_from_fixture_pattern(self):
        """Mirrors the fixture pattern: register a teardown inside a function."""
        cleaned_up: list[str] = []

        def fixture() -> None:
            ipso.register_teardown(lambda: cleaned_up.append("done"))

        fixture()
        assert len(_state.teardown_stack) == 1
        _runner.run_teardowns()
        assert cleaned_up == ["done"]


# ---------------------------------------------------------------------------
# Integration: full test lifecycle
# ---------------------------------------------------------------------------


class TestFullLifecycle:
    def test_simple_test_pattern(self):
        """Simple pattern from the spec: load cell, execute, assert on state."""
        _runner.load_cell("_lc_value = 100")
        ipso.execute_cell()
        import sys

        g = sys._getframe(0).f_globals
        assert g["_lc_value"] == 100
        del g["_lc_value"]
        assert json.loads(_runner.get_test_results()) == []

    def test_data_driven_test_pattern_with_subtests(self):
        """Data-driven pattern: multiple execute_cell calls, one subtest per case."""
        _runner.load_cell("_product = _price * _qty")

        cases = [
            {"price": 2, "qty": 3, "expected": 6},
            {"price": 0, "qty": 5, "expected": 0},
            {"price": 10, "qty": 10, "expected": 100},
        ]

        import sys

        g: dict[str, Any] = sys._getframe(0).f_globals

        for case in cases:
            g["_price"] = case["price"]
            g["_qty"] = case["qty"]
            ipso.execute_cell()
            with ipso.subtest(f"price={case['price']} qty={case['qty']}"):
                assert g["_product"] == case["expected"]

        for key in ("_price", "_qty", "_product"):
            if key in g:
                del g[key]

        results = json.loads(_runner.get_test_results())
        assert len(results) == 3
        assert all(r["passed"] for r in results)

    def test_teardown_fires_after_test(self):
        """Fixture registers teardown; runner calls run_teardowns after test."""
        cleaned: list[str] = []
        ipso.register_teardown(lambda: cleaned.append("cleaned"))
        _runner.run_teardowns()
        assert cleaned == ["cleaned"]
        assert _state.teardown_stack == []

    def test_cell_exception_propagates_without_subtest(self):
        """If cell raises and test doesn't catch it, the exception surfaces."""
        _runner.load_cell("raise TypeError('bad input')")
        with pytest.raises(TypeError, match="bad input"):
            ipso.execute_cell()

    def test_cell_exception_catchable_by_test_code(self):
        """Test code can catch cell exceptions explicitly (try/except pattern)."""
        _runner.load_cell("raise ValueError('expected failure')")
        caught = None
        try:
            ipso.execute_cell()
        except ValueError as e:
            caught = str(e)
        assert caught == "expected failure"

    def test_test_results_empty_when_no_subtest_called(self):
        """If test code never calls subtest(), get_test_results returns empty list."""
        _runner.load_cell("_x = 1")
        ipso.execute_cell()
        import sys

        g = sys._getframe(0).f_globals
        del g["_x"]
        assert json.loads(_runner.get_test_results()) == []

    def test_runner_reads_results_as_json(self):
        """Runner retrieves results via get_test_results() and deserializes them."""
        with ipso.subtest("pass"):
            pass
        with ipso.subtest("fail"):
            raise AssertionError("bad")

        results = json.loads(_runner.get_test_results())
        assert results[0] == {"name": "pass", "passed": True, "error": None, "traceback": None}
        assert results[1]["name"] == "fail"
        assert results[1]["passed"] is False
