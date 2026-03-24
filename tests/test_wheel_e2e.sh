#!/usr/bin/env bash
# End-to-end test: install a nota-bene wheel into a fresh venv and verify
# that both the CLI binary and Python library are usable.
#
# Usage:
#   bash tests/test_wheel_e2e.sh path/to/nota_bene-*.whl
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
    cd "$REPO_ROOT/nota-bene"
    uvx maturin build --release -o "$REPO_ROOT/target/wheels/"
    WHEEL="$(ls "$REPO_ROOT/target/wheels/"nota_bene-*.whl | head -1)"
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

echo "==> Checking nota-bene binary is on PATH"
BINARY="$(which nota-bene)"
echo "    Found: $BINARY"

echo "==> Running nota-bene --help"
nota-bene --help

echo "==> Verifying --help output contains expected text"
HELP_OUTPUT="$(nota-bene --help 2>&1)"
if echo "$HELP_OUTPUT" | grep -qi "usage\|notebook\|nota"; then
    echo "    OK: help output looks correct"
else
    echo "    FAIL: unexpected --help output"
    echo "$HELP_OUTPUT"
    exit 1
fi

# --- Verify Python library ---------------------------------------------------

echo "==> Checking nota_bene Python package is importable"
python -c "import nota_bene; print(f'nota_bene version: {nota_bene.__version__}')"

# --- Done --------------------------------------------------------------------

deactivate
echo ""
echo "==> All e2e checks passed!"
