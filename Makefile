.PHONY: help build clean lint \
        nb-sync nb-sync-locked nb-test nb-lint nb-format nb-fix nb-typing nb-all nb-build \
        test-setup test clean-venv

VENV := tests/.venv
PYTHON := $(VENV)/bin/python
PIP    := $(VENV)/bin/pip

.DEFAULT_GOAL := help

help:
	@echo "nota-bene Makefile targets:"
	@echo ""
	@echo "  make build              Build debug CLI (target/debug/nota-bene)"
	@echo "  make clean              Remove target/"
	@echo "  make lint               Check format (cargo fmt) and run clippy (deny warnings)"
	@echo "  make test               Set up test venv and run cargo test"
	@echo ""
	@echo "  Python package (nota-bene/):"
	@echo "  make nb-sync            Sync Python dev + lint deps"
	@echo "  make nb-sync-locked     Sync Python deps from lockfile"
	@echo "  make nb-test            Run Python tests"
	@echo "  make nb-lint            Run ruff check"
	@echo "  make nb-format          Run ruff format check (dry-run)"
	@echo "  make nb-fix             Auto-fix lint + format in place"
	@echo "  make nb-typing          Run mypy"
	@echo "  make nb-all             lint + format + typing"
	@echo "  make nb-build           Build Python wheel and sdist"
	@echo ""
	@echo "  make help               Show this help"

# ---- Rust integration test environment -------------------------------------

$(PYTHON):
	python3 -m venv $(VENV)

test-setup: $(PYTHON)
	$(PIP) install --quiet nbclient ipykernel ipython
	$(PIP) install --quiet -e nota-bene/

test: test-setup
	cargo test

clean-venv:
	rm -rf $(VENV)

# ---- Rust build / format ---------------------------------------------------

build:
	cargo build

clean:
	rm -rf target

fmt:
	cargo fmt -- --check

clippy:
	cargo clippy -- -D warnings

lint: fmt clippy

# ---- Python sub-package (nota-bene/) ---------------------------------------

nb-sync:
	$(MAKE) -C nota-bene sync

nb-sync-locked:
	$(MAKE) -C nota-bene sync-locked

nb-test:
	$(MAKE) -C nota-bene test

nb-lint:
	$(MAKE) -C nota-bene lint

nb-format:
	$(MAKE) -C nota-bene format

nb-fix:
	$(MAKE) -C nota-bene fix

nb-typing:
	$(MAKE) -C nota-bene typing

nb-all:
	$(MAKE) -C nota-bene all

nb-build:
	$(MAKE) -C nota-bene build
