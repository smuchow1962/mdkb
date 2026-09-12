//! HTTP transport for the MCP server using axum + rmcp streamable HTTP.

use tokio_util::sync::CancellationToken;

use super::McpServer;
use super::common::mcp_router;

/// Run the HTTP MCP server.
pub async fn run_http_server(
    server: McpServer,
    bind: &str,
    token: Option<&str>,
) -> crate::error::Result<()> {
    let cancellation_token = CancellationToken::new();
    let router = mcp_router(server, bind, token, false, cancellation_token.clone());

    let listener = tokio::net::TcpListener::bind(bind)
        .await
        .map_err(|e| crate::error::Error::mcp(format!("Failed to bind to {bind}: {e}")))?;

    tracing::info!("Starting mdkb MCP HTTP server on {bind}");
    eprintln!("mdkb MCP server listening on http://{bind}/mcp");

    axum::serve(listener, router)
        .with_graceful_shutdown(async move {
            if let Err(e) = super::wait_for_shutdown_signal().await {
                tracing::warn!("signal: {e}");
            }
            tracing::info!("Shutdown signal received, stopping server...");
            cancellation_token.cancel();
        })
        .await
        .map_err(|e| crate::error::Error::mcp(format!("HTTP server error: {e}")))?;

    Ok(())
}

#[cfg(test)]
mod tests {
    // Tests are now in common.rs, but we can add HTTP-specific tests here if needed
}
