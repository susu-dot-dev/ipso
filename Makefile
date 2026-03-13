.PHONY: help build clean fmt setup-sample-project e2e-opencode \
        nb-sync nb-sync-locked nb-test nb-lint nb-format nb-fix nb-typing nb-all nb-build

.DEFAULT_GOAL := help

help:
	@echo "nota-bene Makefile targets:"
	@echo ""
	@echo "  make build              Build debug CLI (target/debug/nota-bene)"
	@echo "  make clean              Remove target/ and sample-project/"
	@echo "  make fmt                Check code format (cargo fmt -- --check)"
	@echo "  make setup-sample-project  Create sample-project with opencode.json and nota-bene symlink"
	@echo "  make e2e-opencode        Run functional tests with OpenCode as the tool"
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

build:
	cargo build

clean:
	rm -rf target sample-project

fmt:
	cargo fmt -- --check

setup-sample-project: build
	@mkdir -p sample-project
	@rm -f sample-project/nota-bene
	@ln -sf ../target/debug/nota-bene sample-project/nota-bene
	@API_KEY_REF='{env:OPENROUTER_API_KEY}'; printf '%s\n' "{\"\$$schema\":\"https://opencode.ai/config.json\",\"mcp\":{\"nota-bene\":{\"type\":\"local\",\"command\":[\"./nota-bene\",\"mcp\"],\"enabled\":true}},\"model\":\"openrouter/openrouter/free\",\"provider\":{\"openrouter\":{\"options\":{\"apiKey\":\"$$API_KEY_REF\"}}}}" > sample-project/opencode.json

e2e-opencode: setup-sample-project
	@./scripts/opencode-e2e.sh

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
