.PHONY: help sync test lint format fix typing all build

.DEFAULT_GOAL := help

help:
	@echo "nota-bene Python package targets:"
	@echo ""
	@echo "  make sync           Sync dev + lint deps"
	@echo "  make sync-locked    Sync from lockfile (for CI)"
	@echo "  make test           Run tests"
	@echo "  make lint           Run ruff check"
	@echo "  make format         Run ruff format check (dry-run)"
	@echo "  make fix            Auto-fix lint + format in place"
	@echo "  make typing         Run mypy"
	@echo "  make all            lint + format + typing"
	@echo "  make build          Build wheel and sdist"
	@echo "  make help           Show this help"

sync:
	uv sync

sync-locked:
	uv sync --locked

test:
	uv run pytest tests

lint:
	uv run ruff check .

format:
	uv run ruff format --check .

fix:
	uv run ruff check --fix . && uv run ruff format .

typing:
	uv run mypy src tests

all: lint format typing

build:
	uvx maturin build --release -o dist/
