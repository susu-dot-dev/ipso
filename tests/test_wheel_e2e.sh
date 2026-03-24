#!/usr/bin/env bash
# End-to-end test: install a ipso wheel into a fresh venv and verify
# that both the CLI binary and Python library are usable.
#
# Usage:
#   bash tests/test_wheel_e2e.sh path/to/ipso-*.whl
#
# If no wheel path is given, the script builds one using maturin first.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
VENV_DIR="$(mktemp -d)"

cleanup() {
    rm -rf "$VENV_DIR"
}
trap cleanup EXIT

# --- Determine wheel path ---------------------------------------------------

if [ $# -ge 1 ]; then
    WHEEL="$1"
else
    echo "==> No wheel provided, building with maturin..."
    cd "$REPO_ROOT/ipso"
    uvx maturin build --release -o "$REPO_ROOT/target/wheels/"
    WHEEL="$(ls "$REPO_ROOT/target/wheels/"ipso-*.whl | head -1)"
    cd "$REPO_ROOT"
fi

echo "==> Wheel: $WHEEL"

# --- Create fresh venv and install ------------------------------------------

echo "==> Creating venv in $VENV_DIR"
python3 -m venv "$VENV_DIR"
source "$VENV_DIR/bin/activate"

echo "==> Installing wheel"
pip install --quiet "$WHEEL"

# --- Verify CLI binary -------------------------------------------------------

echo "==> Checking ipso binary is on PATH"
BINARY="$(which ipso)"
echo "    Found: $BINARY"

echo "==> Running ipso --help"
ipso --help

echo "==> Verifying --help output contains expected text"
HELP_OUTPUT="$(ipso --help 2>&1)"
if echo "$HELP_OUTPUT" | grep -qi "usage\|notebook\|ipso"; then
    echo "    OK: help output looks correct"
else
    echo "    FAIL: unexpected --help output"
    echo "$HELP_OUTPUT"
    exit 1
fi

# --- Verify Python library ---------------------------------------------------

echo "==> Checking ipso Python package is importable"
python -c "import ipso; print(f'ipso version: {ipso.__version__}')"

# --- Done --------------------------------------------------------------------

deactivate
echo ""
echo "==> All e2e checks passed!"
