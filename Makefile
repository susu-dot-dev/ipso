.PHONY: help build clean lint test test-setup clean-venv

VENV   := tests/.venv
PYTHON := $(VENV)/bin/python
PIP    := $(VENV)/bin/pip

.DEFAULT_GOAL := help

help:
	@echo "nota-bene Makefile targets:"
	@echo ""
	@echo "  make build      Build debug CLI (target/debug/nota-bene)"
	@echo "  make clean      Remove target/"
	@echo "  make lint       cargo fmt check + clippy"
	@echo "  make test       Run Rust integration tests"
	@echo ""
	@echo "  Python packages: run 'make <target>' inside nota-bene/ or pytest-nota-bene/"
	@echo ""
	@echo "  make help       Show this help"

# ---- Rust integration test environment -------------------------------------

$(PYTHON):
	python3 -m venv $(VENV)

test-setup: $(PYTHON)
	$(PIP) install --quiet -e nota-bene/

test: test-setup
	cargo test

clean-venv:
	rm -rf $(VENV)

# ---- Rust build / lint -----------------------------------------------------

build:
	cargo build

clean:
	rm -rf target

fmt:
	cargo fmt -- --check

clippy:
	cargo clippy -- -D warnings

lint: fmt clippy
