# LSP Server

## Why an LSP Server?

AI coding agents don't understand ipso. They don't know that a cell's test fixtures might be stale, that a diff won't apply cleanly, or that a cell's SHA chain is broken. Without feedback, the AI has no way to know something is wrong — it just keeps working on whatever the user asked for, unaware that the notebook's validation state has degraded.

The LSP server closes this gap. It watches the notebook and pushes diagnostics — structured error and warning messages — back to whatever tool is editing the file. When an AI agent (like OpenCode) receives a diagnostic saying "cell `abc123` is stale: preceding cell was modified," it knows immediately that something needs attention. It didn't have to be told to check. It didn't have to scan the whole notebook. The feedback is automatic and precise.

This is the same mechanism that gives a developer red squiggly lines for a type error in their editor. The difference is that our diagnostics are about notebook-level validity — staleness, broken diffs, missing metadata — rather than syntax or types.

## How the Parts Fit Together

ipso has three interfaces for interacting with notebooks:

1. **CLI** (`ipso status`, `ipso accept`, etc.) — batch commands for humans and scripts
2. **MCP server** — tool-call interface for AI agents that speak MCP
3. **LSP server** — passive diagnostic feedback for any editor or agent that speaks LSP

The LSP server is the **feedback channel**. It doesn't fix anything. It tells the AI (or human) what's wrong. The CLI and MCP server are the **action channels** — they perform the actual operations like accepting cells, updating fixtures, or running tests.

The typical AI workflow:

1. AI edits a notebook cell
2. Editor saves the file
3. LSP server detects the save, analyzes the notebook, and pushes diagnostics
4. AI receives: `[ipso] Cell 'def456' is stale: preceding cell 'abc123' was modified`
5. AI decides what to do — call `ipso accept`, update the fixtures, or investigate further

## Diagnostic Keywords

Each diagnostic carries a machine-readable code — a keyword like `stale`, `missing_sha`, or `diff_conflict`. These keywords serve a dual purpose:

**For programmatic use**: The AI can pattern-match on the code to decide how to respond. A `stale` diagnostic might warrant re-running validation. A `diff_conflict` might mean the diff needs to be regenerated.

**For self-service help**: The AI (or a human) can run `ipso help <keyword>` to get a detailed explanation of what the error means, why it happens, and how to fix it — either manually, via the CLI, or via an MCP tool call. This makes the diagnostic system self-documenting. The AI doesn't need to be pre-trained on ipso's semantics; it can look them up on demand.

This is not implemented yet, but the diagnostic codes are designed with this in mind. When `ipso help` is added, the LSP diagnostics become entry points into the full help system — each squiggly line is a pointer to a specific, actionable explanation.

## What the LSP Server Does Not Do

The LSP server is read-only. It parses the notebook, checks validity, and reports problems. It does not modify files, accept cells, or regenerate fixtures. Those actions belong to the CLI and MCP server.

It also does not try to understand the notebook mid-edit. It only analyzes on file open and file save — moments when the JSON is complete and valid. There is no character-by-character analysis, no incremental parsing, and no debouncing. The computation is fast enough (~3ms) that this simplicity costs nothing.
