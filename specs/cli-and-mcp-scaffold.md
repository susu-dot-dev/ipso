---
name: Ipso CLI and MCP scaffold
overview: Scaffold a Rust binary with a clap-based CLI (--version + `mcp` subcommand) and an MCP server using the official rmcp crate, exposing a single "greet" tool over stdio.
todos: []
isProject: false
---

# Ipso CLI and MCP server scaffold

## Crate selection

### CLI crates considered


| Crate         | Description                                                                                                  | Why chosen / not chosen                                                                                  |
| ------------- | ------------------------------------------------------------------------------------------------------------ | -------------------------------------------------------------------------------------------------------- |
| **clap**      | De facto standard for CLI parsing; derive and builder APIs, subcommands, auto --version/--help, completions. | **Chosen.** Best ecosystem fit, `--version` and subcommands out of the box, widely used and maintained.  |
| **gumdrop**   | Derive-based parser, minimal deps.                                                                           | Lighter than clap but fewer features and smaller ecosystem; no strong reason to prefer for this project. |
| **structopt** | Derive layer on top of clap (now merged into clap's derive API).                                             | Deprecated in favor of clap's built-in derive; use clap directly.                                        |
| **argh**      | Compact derive-based CLI, used in Fuchsia.                                                                   | Good for small CLIs but less flexible for subcommands and version handling than clap.                    |
| **lexopt**    | Minimal, no derive, manual parsing.                                                                          | Too low-level for a small but structured CLI with subcommands.                                           |


**Choice: clap** (with `derive` feature). Fits the need for a binary with `--version` and a `mcp` subcommand with minimal boilerplate and room to grow.

### MCP server crates considered


| Crate                               | Description                                                                                                                                                                                                                                             | Why chosen / not chosen                                                                                                                                |
| ----------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------ |
| **rmcp**                            | Official [Model Context Protocol Rust SDK](https://github.com/modelcontextprotocol/rust-sdk) (modelcontextprotocol org). Server + client, tools/resources/prompts, stdio and other transports, `#[tool_router]` / `#[tool]` / `#[tool_handler]` macros. | **Chosen.** Official SDK, active maintenance, 1.x stable, macros for tools, stdio transport, and alignment with the MCP spec and Cursor/agent tooling. |
| **model-context-protocol** (tsharp) | Community crate on crates.io; `McpStdioServer`, `#[mcp_tool]`, `tools![]`, groups.                                                                                                                                                                      | Not the official SDK; smaller ecosystem and different maintainer; would duplicate effort if the spec or tooling evolves around the official SDK.       |
| **mcp-protocol-sdk**                | Alternative MCP implementation; multiple transports, type-safe.                                                                                                                                                                                         | Less visibility and adoption than the official rust-sdk; prefer official rmcp for long-term compatibility.                                             |
| **mcp-core** (stevohuncho)          | Focus on efficiency/scalability; STDIO and SSE.                                                                                                                                                                                                         | Alternative implementation rather than the reference SDK; fewer examples and tooling integrations.                                                     |
| **mcp_sdk_rs**                      | Async with Tokio, WebSocket/stdio.                                                                                                                                                                                                                      | Less mainstream than rmcp; official SDK preferred for spec compliance and tooling.                                                                     |


**Choice: rmcp** (1.x, default or `server` + `macros` features). Official MCP Rust SDK, procedural macros for defining tools (e.g. greet with a string argument), stdio transport suitable for Cursor/MCP clients, and a single canonical dependency for MCP in Rust.

## Layout

- **Binary**: single crate `ipso` at workspace root (no workspace; one `Cargo.toml` and `src/`).
- **Entry**: [src/main.rs](src/main.rs) — parse CLI, dispatch to `mcp` or exit after `--version`.
- **MCP server**: [src/mcp.rs](src/mcp.rs) — server struct, greet tool, run over stdio.

## Dependencies

**[Cargo.toml](Cargo.toml)** (new):

- **clap** with `derive` — CLI (version, subcommands). Use `crate_version!()` for `--version`.
- **rmcp** (1.x) — official MCP Rust SDK. Use default features (includes `server`, `macros`) so we get `ServerHandler`, `#[tool_router]`, `#[tool]`, `#[tool_handler]`, and stdio transport.
- **tokio** with `full` or `rt-multi-thread` + `macros` — async runtime for MCP server.
- **serde**, **schemars** — for greet tool argument struct (rmcp's `Parameters<T>` typically needs `JsonSchema`).

## CLI design

- **Global**: `--version` / `-V` prints version and exits (clap's built-in).
- **Subcommand**: `mcp` — no args; runs the MCP server (stdio). No `--help`-only behavior needed beyond clap default.

```text
ipso --version    → print version
ipso mcp          → run MCP server on stdio
```

## MCP server (rmcp)

- **Transport**: stdio. Use rmcp's stdio transport (e.g. `rmcp::transport::stdio::stdio()` or the pattern from the README: `(stdin(), stdout())` and the type expected by `ServiceExt::serve`).
- **Server struct**: e.g. `IpsoMcp` (or `McpServer`), holding a `ToolRouter<Self>`.
- **Tool "greet"**:
  - One argument: string (e.g. `name: String`). Define a small struct `GreetParams { name: String }` with `Serialize`, `Deserialize`, `JsonSchema` and use `Parameters<GreetParams>` in the tool method.
  - Return: `"Hello, <name>"` (e.g. return type `String` if the macro supports it, or build `CallToolResult` with text content per rmcp API).
- **Macros**: `#[tool_router]` on the impl that defines the tool, `#[tool(description = "...")]` on the greet method, `#[tool_handler]` on the `ServerHandler` impl that delegates to the router. Implement `ServerHandler::get_info()` (and any other required methods) and enable tools capability.
- **Run**: In `main`, when subcommand is `mcp`: build server, create stdio transport, call `server.serve(transport).await?`, then `server.waiting().await?` (or equivalent) to keep running until shutdown.

## File summary


| File                       | Purpose                                                                                                                                                                                                     |
| -------------------------- | ----------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| [Cargo.toml](Cargo.toml)   | Package manifest, dependencies (clap, rmcp, tokio, serde, schemars).                                                                                                                                        |
| [src/main.rs](src/main.rs) | Clap CLI; handle `--version` and `mcp`; for `mcp`, call into `mcp::run()` (or similar).                                                                                                                     |
| [src/mcp.rs](src/mcp.rs)   | MCP server type, greet tool (Parameters → "Hello, {name}"), ServerHandler + tool_router/tool_handler; `pub async fn run() -> Result<(), Box<dyn std::error::Error>>` that builds transport and runs server. |


## Implementation notes

- **rmcp docs**: Follow the official [rust-sdk](https://github.com/modelcontextprotocol/rust-sdk) README and [tool_router / tool / tool_handler](https://docs.rs/rmcp-macros/latest/rmcp_macros/attr.tool.html) docs. If the macro expects a different return type (e.g. `CallToolResult`), return that and construct text content with the crate's helper (e.g. `CallToolResult::success(vec![Content::text(...)])`).
- **Stdio**: Use rmcp's recommended stdio API so the server is driven by stdin/stdout (e.g. `tokio::io::stdin()`/`stdout()` or the wrapper exposed by rmcp).
- **Version**: Set `version` in `Cargo.toml` (e.g. `0.1.0`); clap's `crate_version!()` will pick it up for `--version`.
