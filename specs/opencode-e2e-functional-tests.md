---
name: OpenCode E2E functional tests
overview: "Add a Makefile and sample-project for local OpenCode E2E testing: build the ipso CLI, set up a project with opencode.json pointing at the local MCP server, and a script that runs opencode in non-interactive mode and validates the MCP tool was invoked via JSON output."
todos: []
isProject: false
---

# OpenCode E2E functional tests for ipso

## Context

- **CLI/MCP**: Single binary `ipso`; `ipso mcp` runs the MCP server on stdio. Tool: `greet(name: string)` → `"Hello, {name}"` ([src/mcp.rs](src/mcp.rs)).
- **OpenCode**: Uses [opencode.json](https://opencode.ai/docs/mcp-servers/) with `mcp.<name>.type: "local"` and `command: [path, "mcp"]`. Non-interactive run: `opencode run "prompt"`; `--format json` emits JSONL with a `tool_use` event containing tool name, input, and output.

## 1. Add `sample-project` to .gitignore

Append `sample-project` (or `sample-project/`) to [.gitignore](.gitignore) so the generated test project is not committed.

## 2. Makefile

Create a **Makefile** at repo root with:

- **`make build`**  
  - Run `cargo build` (debug).  
  - Artifact: `target/debug/ipso`.
- **`make clean`**  
  - Remove `target/` and `sample-project/` (e.g. `rm -rf target sample-project`). Restores repo to pre-setup state.
- **`make setup-sample-project`** (depends on build)
  - Create directory `sample-project/`.
  - Symlink: `sample-project/ipso` → `../target/debug/ipso` (relative from repo root so the link is portable).
  - Write `sample-project/opencode.json` with OpenCode MCP config:
    - Schema: `https://opencode.ai/config.json`.
    - `mcp.ipso`: `type: "local"`, `command: ["./ipso", "mcp"]`, `enabled: true`.
  - OpenCode resolves config from CWD, so when we `cd sample-project` and run `opencode run ...`, it will use this config and start the MCP server via `./ipso mcp`.
- **`make e2e-opencode`** (depends on setup-sample-project)
  - Invoke `./scripts/opencode-e2e.sh` from repo root. Prereqs (build, sample-project) are guaranteed by make dependencies; the script does not check them.

Use a single top-level target (e.g. `setup-sample-project`) that runs `build` first (dependency or explicit `$(MAKE) build`), then creates the dir, symlink, and config.

## 3. E2E script: `scripts/opencode-e2e.sh`

Create **scripts/** and **scripts/opencode-e2e.sh**. No prereq checks in the script—**`make e2e-opencode`** depends on **setup-sample-project**, so the caller (make) ensures build and sample-project exist.

- **CWD**: `cd` to `sample-project` (e.g. `cd "$(dirname "$0")/../sample-project"`).
- **Run OpenCode**:  
`opencode run "Use the greet tool with the name sunshine" --format json`  
Capture stdout (and optionally stderr) to a temp file or variable.
- **Validation (Option A — parse JSONL)**:  
OpenCode's `--format json` output is JSONL; one event type is **tool_use**, which includes the tool result (e.g. text "Hello, sunshine"). Read the captured output line-by-line as JSONL. Find an event where `type` is `"tool_use"` (or the name OpenCode uses for tool-result events), the tool name is `greet`, and the payload contains `"Hello, sunshine"` (e.g. in `result.content[].text` or equivalent). Use **jq** to parse each line and assert that at least one line has the expected type and output text. Success = such an event exists; failure = no such event or parse error.
- **Exit code**: Exit 0 if validation passes, non-zero otherwise; print a clear success/failure message.
- **Timeout**: Use a reasonable timeout for `opencode run` (e.g. `timeout 120` or env-based) so CI doesn't hang.

## 4. Validation approach summary


| Method                       | Pros                                                                        | Cons                                             |
| ---------------------------- | --------------------------------------------------------------------------- | ------------------------------------------------ |
| **grep "Hello, sunshine"**   | No JSON parsing; works if OpenCode embeds tool output in stdout.            | Brittle if output format changes.                |
| **Parse JSONL for tool_use** | Robust; explicitly checks that a tool call completed with the right result. | Slightly more script logic (jq or small parser). |


**Chosen approach:** Implement **Option A**: parse JSONL with jq, find a line where `.type == "tool_use"` (or OpenCode's equivalent), the tool is **greet**, and the tool result content contains "Hello, sunshine".

## 5. Make target e2e-opencode

**`make e2e-opencode`** runs `./scripts/opencode-e2e.sh` from the repo root. It depends on **setup-sample-project** (which depends on **build**), so running `make e2e-opencode` ensures prereqs are met before the script runs.

## 6. Copy plan to specs/

When implementing, copy this planning document to **specs/** in the repo (e.g. `specs/opencode-e2e-functional-tests.md`) so the design is versioned with the code.

## File summary

- [.gitignore](.gitignore): append `sample-project`
- **Makefile** (new): `build`, `clean`, `setup-sample-project`, `e2e-opencode` (depends on setup-sample-project)
- **scripts/opencode-e2e.sh** (new): cd to sample-project, run opencode with prompt to use the **greet** tool with name "sunshine", validate JSONL for a greet tool_use event with "Hello, sunshine"; no prereq logic
- **specs/opencode-e2e-functional-tests.md** (new): copy of this plan, created during implementation

## Notes

- The prompt instructs OpenCode to "use the **greet** tool with the name sunshine". The ipso MCP server exposes the `greet(name)` tool; the LLM should call `greet` with `name: "sunshine"` and get "Hello, sunshine". No changes to the Rust MCP code are required.
- If OpenCode is not installed, `opencode run` will fail with the shell's usual "command not found"; no extra checks needed in the script.
