//! stella-mcp: an MCP server to run and debug Atari 2600 (VCS) games.
//!
//! The server drives a headless build of the Gopher2600 debugger over a framed
//! stdin/stdout protocol, exposing an interactive debugger as MCP tools. It
//! speaks MCP over stdio; diagnostics go to stderr so they never corrupt the
//! protocol stream.

mod emulator;
mod server;
mod session;

use anyhow::Result;
use rmcp::ServiceExt;
use rmcp::transport::stdio;
use tracing_subscriber::EnvFilter;

use crate::emulator::Toolchain;
use crate::server::StellaMcp;

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .with_writer(std::io::stderr)
        .with_ansi(false)
        .init();

    let toolchain = Toolchain::discover();
    tracing::info!(root = %toolchain.root.display(), helper = ?toolchain.helper, "starting stella-mcp");

    let service = StellaMcp::new(toolchain)
        .serve(stdio())
        .await
        .inspect_err(|e| tracing::error!("failed to start server: {e:?}"))?;

    service.waiting().await?;
    Ok(())
}
