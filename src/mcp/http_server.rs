//! HTTP transport for the MCP server using axum + rmcp streamable HTTP.

use std::sync::Arc;

use axum::Router;
use axum::middleware;
use rmcp::transport::streamable_http_server::StreamableHttpService;
use rmcp::transport::streamable_http_server::session::local::LocalSessionManager;
use rmcp::transport::streamable_http_server::tower::StreamableHttpServerConfig;
use tokio_util::sync::CancellationToken;

use super::McpServer;
use super::common::{AppState, auth_middleware, health_handler};

/// Run the HTTP MCP server.
pub async fn run_http_server(
    server: McpServer,
    bind: &str,
    token: Option<&str>,
) -> crate::error::Result<()> {
    let cancellation_token = CancellationToken::new();

    let config = StreamableHttpServerConfig {
        stateful_mode: true,
        cancellation_token: cancellation_token.clone(),
        ..Default::default()
    };

    let session_manager = Arc::new(LocalSessionManager::default());

    let mcp_service =
        StreamableHttpService::new(move || Ok(server.clone()), session_manager, config);

    let state = AppState {
        token: token.map(String::from),
    };

    let router = Router::new()
        .route("/health", axum::routing::get(|| health_handler(false)))
        .nest_service("/mcp", mcp_service)
        .layer(middleware::from_fn_with_state(
            state.clone(),
            auth_middleware,
        ))
        .with_state(state);

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
