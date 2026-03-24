.PHONY: help build clean lint test test-setup clean-venv test-wheel

VENV   := tests/.venv
PYTHON := $(VENV)/bin/python
PIP    := $(VENV)/bin/pip

.DEFAULT_GOAL := help

help:
	@echo "ipso Makefile targets:"
	@echo ""
	@echo "  make build        Build debug CLI (target/debug/ipso)"
	@echo "  make clean        Remove target/"
	@echo "  make lint         cargo fmt check + clippy"
	@echo "  make test         Run Rust integration tests"
	@echo "  make test-wheel   E2E: build wheel, install in fresh venv, verify CLI"
	@echo ""
	@echo "  Python packages: run 'make <target>' inside ipso/ or pytest-ipso/"
	@echo ""
	@echo "  make help         Show this help"

# ---- Rust integration test environment -------------------------------------

$(PYTHON):
	python3 -m venv $(VENV)

test-setup: $(PYTHON)
	$(PIP) install --quiet -e ipso/

test: test-setup
	cargo test

clean-venv:
	rm -rf $(VENV) $(WHEEL_VENV)

# ---- E2E wheel test -------------------------------------------------------

test-wheel:
	@echo "==> Building wheel with maturin..."
	cd ipso && uvx maturin build --release -o ../target/wheels/
	@echo "==> Running e2e test..."
	bash tests/test_wheel_e2e.sh $$(ls target/wheels/ipso-*.whl | head -1)

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
