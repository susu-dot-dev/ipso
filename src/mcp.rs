use rmcp::{
    handler::server::{router::tool::ToolRouter, wrapper::Parameters},
    model::*,
    schemars,
    service::ServiceExt,
    tool, tool_handler, tool_router,
    transport::io,
    ErrorData as McpError, ServerHandler,
};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
pub struct GreetParams {
    /// Name to greet.
    pub name: String,
}

#[derive(Clone)]
pub struct NotaBeneMcp {
    tool_router: ToolRouter<Self>,
}

#[tool_router]
impl NotaBeneMcp {
    pub fn new() -> Self {
        Self {
            tool_router: Self::tool_router(),
        }
    }

    #[tool(description = "Greet someone by name")]
    async fn greet(&self, Parameters(params): Parameters<GreetParams>) -> Result<CallToolResult, McpError> {
        Ok(CallToolResult::success(vec![Content::text(format!("Hello, {}", params.name))]))
    }
}

#[tool_handler]
impl ServerHandler for NotaBeneMcp {
    fn get_info(&self) -> ServerInfo {
        ServerInfo::new(
            ServerCapabilities::builder()
                .enable_tools()
                .build(),
        )
        .with_protocol_version(ProtocolVersion::V_2025_06_18)
    }
}

pub async fn run() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let server = NotaBeneMcp::new();
    let transport = io::stdio();
    let running = server.serve(transport).await?;
    running.waiting().await?;
    Ok(())
}
