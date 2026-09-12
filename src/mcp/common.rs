//! Shared types and middleware for HTTP/HTTPS MCP servers.

use std::net::SocketAddr;
use std::sync::Arc;

use axum::Router;
use axum::body::Body;
use axum::extract::State;
use axum::http::{Request, StatusCode, header};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Json, Response};
use rmcp::transport::streamable_http_server::StreamableHttpService;
use rmcp::transport::streamable_http_server::session::local::LocalSessionManager;
use rmcp::transport::streamable_http_server::tower::StreamableHttpServerConfig;
use subtle::ConstantTimeEq;
use tokio_util::sync::CancellationToken;

use super::McpServer;

/// Shared state for middleware.
#[derive(Clone, Debug)]
pub struct AppState {
    pub token: Option<String>,
}

/// Health check endpoint.
///
/// Returns JSON with status, version, and optional TLS flag.
pub async fn health_handler(tls_enabled: bool) -> impl IntoResponse {
    let mut response = serde_json::json!({
        "status": "ok",
        "version": env!("CARGO_PKG_VERSION"),
    });

    if tls_enabled {
        response["tls"] = serde_json::json!(true);
    }

    Json(response)
}

/// The router both network transports serve: `/health` open, the rmcp
/// streamable-HTTP endpoint at `/mcp` behind the bearer-token middleware.
///
/// rmcp validates the `Host` header of every `/mcp` request against
/// [`allowed_hosts`] and answers 403 to any other value. That is the
/// DNS-rebinding guard (RUSTSEC-2026-0189): a web page the operator visits
/// cannot reach this server through a name it controls.
pub fn mcp_router(
    server: McpServer,
    bind: &str,
    token: Option<&str>,
    tls_enabled: bool,
    cancellation_token: CancellationToken,
) -> Router {
    let config = StreamableHttpServerConfig::default()
        .with_legacy_session_mode(true)
        .with_cancellation_token(cancellation_token)
        .with_allowed_hosts(allowed_hosts(bind));

    let session_manager = Arc::new(LocalSessionManager::default());

    let mcp_service =
        StreamableHttpService::new(move || Ok(server.clone()), session_manager, config);

    let state = AppState {
        token: token.map(String::from),
    };

    Router::new()
        .route(
            "/health",
            axum::routing::get(move || health_handler(tls_enabled)),
        )
        .nest_service("/mcp", mcp_service)
        .layer(middleware::from_fn_with_state(
            state.clone(),
            auth_middleware,
        ))
        .with_state(state)
}

/// The `Host` values rmcp accepts: its loopback defaults plus the concrete
/// address in `bind`, so a server bound to one LAN address answers clients
/// that name that address. A wildcard bind (`0.0.0.0`, `[::]`) names no
/// address, so it adds nothing and such a server answers loopback clients
/// only. Entries carry no port: rmcp then accepts any port for that host.
pub fn allowed_hosts(bind: &str) -> Vec<String> {
    let mut hosts = StreamableHttpServerConfig::default().allowed_hosts;
    let bound = match bind.parse::<SocketAddr>() {
        Ok(addr) if addr.ip().is_unspecified() => None,
        Ok(addr) => Some(addr.ip().to_string()),
        Err(_) => bind.rsplit_once(':').map(|(host, _)| host.to_string()),
    };
    if let Some(host) = bound
        && !hosts.contains(&host)
    {
        hosts.push(host);
    }
    hosts
}

/// Bearer token authentication middleware.
///
/// If no token is configured, all requests are allowed.
/// The /health endpoint is always accessible without auth.
pub async fn auth_middleware(
    State(state): State<AppState>,
    request: Request<Body>,
    next: Next,
) -> Response {
    // Always allow health checks without auth
    if request.uri().path() == "/health" {
        return next.run(request).await;
    }

    // If no token configured, allow all requests
    let Some(expected_token) = &state.token else {
        return next.run(request).await;
    };

    // Check Authorization header
    let auth_header = request
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok());

    match auth_header {
        Some(auth) if auth.starts_with("Bearer ") => {
            let provided = &auth.as_bytes()["Bearer ".len()..];
            let expected = expected_token.as_bytes();
            if provided.ct_eq(expected).into() {
                next.run(request).await
            } else {
                (StatusCode::UNAUTHORIZED, "Invalid bearer token").into_response()
            }
        }
        _ => (StatusCode::UNAUTHORIZED, "Bearer token required").into_response(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tower::ServiceExt;

    /// Build a test router with auth middleware and a simple OK handler.
    fn test_router(token: Option<&str>) -> Router {
        let state = AppState {
            token: token.map(String::from),
        };
        Router::new()
            .route("/health", axum::routing::get(|| health_handler(false)))
            .route("/mcp", axum::routing::get(|| async { StatusCode::OK }))
            .layer(middleware::from_fn_with_state(
                state.clone(),
                auth_middleware,
            ))
            .with_state(state)
    }

    /// Send a request to the router and return the response status.
    async fn send_request(router: Router, uri: &str, auth_header: Option<&str>) -> StatusCode {
        let mut req_builder = Request::builder().uri(uri).method("GET");
        if let Some(auth) = auth_header {
            req_builder = req_builder.header(header::AUTHORIZATION, auth);
        }
        let request = req_builder.body(Body::empty()).unwrap();
        let response = router.oneshot(request).await.unwrap();
        response.status()
    }

    /// The loopback names rmcp ships stay on every list: a browser on the
    /// same machine reaches the server as `localhost` whatever it is bound as.
    fn assert_loopback(hosts: &[String]) {
        for name in ["localhost", "127.0.0.1", "::1"] {
            assert!(
                hosts.iter().any(|h| h == name),
                "{name} missing from {hosts:?}"
            );
        }
    }

    #[test]
    fn allowed_hosts_default_bind_is_loopback_only() {
        let hosts = allowed_hosts("127.0.0.1:8080");
        assert_loopback(&hosts);
        assert_eq!(hosts.len(), 3, "loopback bind adds no duplicate: {hosts:?}");
    }

    #[test]
    fn allowed_hosts_lan_bind_adds_that_address() {
        let hosts = allowed_hosts("192.168.1.20:8080");
        assert_loopback(&hosts);
        assert!(hosts.contains(&"192.168.1.20".to_string()), "{hosts:?}");
    }

    #[test]
    fn allowed_hosts_ipv6_bind_adds_bare_address() {
        // rmcp strips the brackets from the `Host` header before matching,
        // so the entry must be the bare address.
        let hosts = allowed_hosts("[fd00::7]:8080");
        assert!(hosts.contains(&"fd00::7".to_string()), "{hosts:?}");
    }

    #[test]
    fn allowed_hosts_wildcard_bind_adds_nothing() {
        for bind in ["0.0.0.0:8080", "[::]:8080"] {
            let hosts = allowed_hosts(bind);
            assert_loopback(&hosts);
            assert_eq!(
                hosts.len(),
                3,
                "{bind} must not allow every Host: {hosts:?}"
            );
        }
    }

    #[test]
    fn allowed_hosts_hostname_bind_adds_the_name() {
        let hosts = allowed_hosts("mdkb.internal:8080");
        assert!(hosts.contains(&"mdkb.internal".to_string()), "{hosts:?}");
    }

    #[tokio::test]
    async fn test_auth_no_token_configured_allows_all() {
        let router = test_router(None);
        assert_eq!(send_request(router, "/mcp", None).await, StatusCode::OK);
    }

    #[tokio::test]
    async fn test_auth_health_bypasses_token_check() {
        let router = test_router(Some("secret"));
        assert_eq!(send_request(router, "/health", None).await, StatusCode::OK);
    }

    #[tokio::test]
    async fn test_auth_valid_token_allowed() {
        let router = test_router(Some("secret"));
        assert_eq!(
            send_request(router, "/mcp", Some("Bearer secret")).await,
            StatusCode::OK
        );
    }

    #[tokio::test]
    async fn test_auth_invalid_token_rejected() {
        let router = test_router(Some("secret"));
        assert_eq!(
            send_request(router, "/mcp", Some("Bearer wrong")).await,
            StatusCode::UNAUTHORIZED
        );
    }

    #[tokio::test]
    async fn test_auth_missing_header_rejected() {
        let router = test_router(Some("secret"));
        assert_eq!(
            send_request(router, "/mcp", None).await,
            StatusCode::UNAUTHORIZED
        );
    }

    #[tokio::test]
    async fn test_auth_non_bearer_scheme_rejected() {
        let router = test_router(Some("secret"));
        assert_eq!(
            send_request(router, "/mcp", Some("Basic dXNlcjpwYXNz")).await,
            StatusCode::UNAUTHORIZED
        );
    }
}
