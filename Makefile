.PHONY: help build clean setup-sample-project e2e-opencode

.DEFAULT_GOAL := help

help:
	@echo "nota-bene Makefile targets:"
	@echo ""
	@echo "  make build              Build debug CLI (target/debug/nota-bene)"
	@echo "  make clean              Remove target/ and sample-project/"
	@echo "  make setup-sample-project  Create sample-project with opencode.json and nota-bene symlink"
	@echo "  make e2e-opencode        Run functional tests with OpenCode as the tool"
	@echo "  make help               Show this help"

build:
	cargo build

clean:
	rm -rf target sample-project

setup-sample-project: build
	@mkdir -p sample-project
	@rm -f sample-project/nota-bene
	@ln -sf ../target/debug/nota-bene sample-project/nota-bene
	@printf '%s\n' '{"$$schema":"https://opencode.ai/config.json","mcp":{"nota-bene":{"type":"local","command":["./nota-bene","mcp"],"enabled":true}}}' > sample-project/opencode.json

e2e-opencode: setup-sample-project
	@./scripts/opencode-e2e.sh
