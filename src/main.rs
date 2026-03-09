mod mcp;

use clap::Parser;

#[derive(Parser)]
#[command(name = "nota-bene", version = env!("CARGO_PKG_VERSION"))]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(clap::Subcommand)]
enum Command {
    /// Start the MCP server (stdio transport).
    Mcp,
}

fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let cli = Cli::parse();
    match cli.command {
        None => {
            // No subcommand: clap already handled --version / -V
            Ok(())
        }
        Some(Command::Mcp) => {
            let rt = tokio::runtime::Runtime::new()?;
            rt.block_on(mcp::run())
        }
    }
}
