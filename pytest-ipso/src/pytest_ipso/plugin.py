"""pytest plugin for ipso notebook cell tests.

Discovers .ipynb files, shells out to the ipso CLI to run tests,
and maps results back to pytest's collection and reporting system.
"""

from __future__ import annotations

import json
import subprocess
from pathlib import Path
from typing import Any

import pytest


# ---------------------------------------------------------------------------
# Exception classes
# ---------------------------------------------------------------------------


class IpsoTestError(Exception):
    """Raised when the ipso CLI reports an infrastructure error."""

    def __init__(self, error: dict[str, Any]) -> None:
        self.ipso_error = error
        super().__init__(str(error))


class IpsoSubtestFailure(Exception):
    """Raised when one or more subtests fail."""

    def __init__(self, subtests: list[dict[str, Any]]) -> None:
        self.subtests = subtests
        super().__init__(f"{sum(1 for s in subtests if not s.get('passed'))} subtest(s) failed")


# ---------------------------------------------------------------------------
# Configuration
# ---------------------------------------------------------------------------


def pytest_addoption(parser: pytest.Parser) -> None:
    group = parser.getgroup("ipso", "ipso notebook test options")
    group.addoption(
        "--ipso-timeout",
        default=60,
        type=int,
        help="Timeout in seconds passed to ipso test --timeout (default: 60)",
    )


# ---------------------------------------------------------------------------
# Collection
# ---------------------------------------------------------------------------


def pytest_collect_file(parent: pytest.Collector, file_path: Path) -> pytest.Collector | None:
    if file_path.suffix == ".ipynb":
        return IpsoNotebook.from_parent(parent, path=file_path)
    return None


class IpsoNotebook(pytest.File):
    """Collector for a single .ipynb file."""

    def collect(self) -> list[pytest.Item]:
        try:
            notebook = json.loads(self.path.read_text(encoding="utf-8"))
        except Exception as exc:
            raise pytest.UsageError(f"Failed to read notebook {self.path}: {exc}") from exc

        items: list[pytest.Item] = []
        for cell in notebook.get("cells", []):
            ipso_meta = cell.get("metadata", {}).get("ipso", {})
            test_meta = ipso_meta.get("test")
            if test_meta is None:
                continue
            cell_id: str = cell.get("id", "")
            test_name: str = test_meta.get("name", cell_id)
            items.append(
                IpsoCellTest.from_parent(
                    self,
                    name=test_name,
                    cell_id=cell_id,
                    test_name=test_name,
                )
            )
        return items


class IpsoCellTest(pytest.Item):
    """A single cell test item."""

    def __init__(
        self,
        *,
        name: str,
        parent: pytest.Collector,
        cell_id: str,
        test_name: str,
    ) -> None:
        super().__init__(name=name, parent=parent)
        self.cell_id = cell_id
        self.test_name = test_name

    def runtest(self) -> None:
        config = self.config
        timeout: int = config.getoption("--ipso-timeout")

        cmd = [
            "ipso",
            "test",
            str(self.path),
            "--filter",
            f"cell:{self.cell_id}",
            "--timeout",
            str(timeout),
        ]

        try:
            result = subprocess.run(cmd, capture_output=True, text=True)
        except FileNotFoundError as exc:
            raise IpsoTestError(
                {
                    "phase": "invocation",
                    "detail": "ipso binary not found. Ensure the 'ipso' package is installed.",
                    "traceback": "",
                }
            ) from exc

        # Parse JSON output
        try:
            cell_results: list[dict[str, Any]] = json.loads(result.stdout)
        except json.JSONDecodeError as exc:
            raise IpsoTestError(
                {
                    "phase": "parse",
                    "detail": f"Failed to parse ipso output: {exc}\nstdout: {result.stdout}\nstderr: {result.stderr}",
                    "traceback": "",
                }
            ) from exc

        if not cell_results:
            raise IpsoTestError(
                {
                    "phase": "execution",
                    "detail": "ipso returned no results",
                    "traceback": "",
                }
            )

        cell_result = cell_results[0]

        if result.returncode == 2 or cell_result.get("status") == "error":
            raise IpsoTestError(
                cell_result.get("error", {"phase": "unknown", "detail": result.stderr, "traceback": ""})
            )

        subtests: list[dict[str, Any]] = cell_result.get("subtests", [])
        if any(not s.get("passed") for s in subtests):
            raise IpsoSubtestFailure(subtests)

    def repr_failure(self, excinfo: pytest.ExceptionInfo[BaseException], style: Any = None) -> str:
        exc = excinfo.value
        if isinstance(exc, IpsoTestError):
            err = exc.ipso_error
            lines = [f"Infrastructure error in phase: {err.get('phase', 'unknown')}"]
            if err.get("fixture_name"):
                lines.append(f"  fixture: {err['fixture_name']}")
            if err.get("source_cell_id"):
                lines.append(f"  source cell: {err['source_cell_id']}")
            if err.get("detail"):
                lines.append(f"  detail: {err['detail']}")
            if err.get("traceback"):
                lines.append("")
                lines.append(err["traceback"])
            return "\n".join(lines)

        if isinstance(exc, IpsoSubtestFailure):
            lines = []
            for st in exc.subtests:
                status = "PASSED" if st.get("passed") else "FAILED"
                lines.append(f"  {status} {st.get('name', '')}")
                if not st.get("passed"):
                    if st.get("error"):
                        lines.append(f"    {st['error']}")
                    if st.get("traceback"):
                        lines.append("")
                        lines.append(st["traceback"])
            return "\n".join(lines)

        return str(excinfo.value)

    def reportinfo(self) -> tuple[Path, int | None, str]:
        return self.path, None, f"{self.path.name}::{self.test_name}"
