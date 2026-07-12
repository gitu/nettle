//! Optional local HTTP control server.
//!
//! Off by default. When enabled it serves a token-authorized web panel and a
//! JSON API that drives the *same* session machinery as the desktop UI — so you
//! can browse and move files on your connected hosts, and connect/disconnect or
//! toggle port-forwards, from a phone or another browser via a link the app
//! hands out. The token is a shared secret embedded in that link; it is the
//! only thing standing between the network and your hosts, so the server binds
//! to localhost unless you explicitly opt into LAN exposure.

use std::net::{IpAddr, SocketAddr};

use axum::{
    body::{Body, Bytes},
    extract::{DefaultBodyLimit, Query, Request, State},
    http::{header, StatusCode},
    middleware::{from_fn_with_state, Next},
    response::{Html, IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use serde::Deserialize;
use serde_json::{json, Value};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::config::WebConfig;
use crate::error::NettleError;
use crate::ipc::commands::{connect_host, disconnect_host, with_session};
use crate::ipc::types::ForwardInfo;
use crate::state::AppState;

/// A running control server. Dropping/`stop`-ing it shuts the server down.
pub struct WebHandle {
    shutdown: CancellationToken,
    join: tokio::task::JoinHandle<()>,
    pub bound: SocketAddr,
}

impl WebHandle {
    /// Signal graceful shutdown and wait briefly for in-flight requests.
    pub async fn stop(self) {
        self.shutdown.cancel();
        let _ = tokio::time::timeout(std::time::Duration::from_secs(3), self.join).await;
    }
}

#[derive(Clone)]
struct Ctx {
    app: AppState,
    token: String,
}

/// Bind and start the server. Returns an error if the port can't be bound.
pub async fn start(app: AppState, cfg: &WebConfig) -> Result<WebHandle, NettleError> {
    let ip = if cfg.lan {
        IpAddr::from([0, 0, 0, 0])
    } else {
        IpAddr::from([127, 0, 0, 1])
    };
    let addr = SocketAddr::new(ip, cfg.port);
    let listener = tokio::net::TcpListener::bind(addr)
        .await
        .map_err(|e| NettleError::Msg(format!("cannot bind {addr}: {e}")))?;
    let bound = listener
        .local_addr()
        .map_err(|e| NettleError::Msg(e.to_string()))?;

    let ctx = Ctx {
        app,
        token: cfg.token.clone(),
    };
    let router = build_router(ctx);

    let shutdown = CancellationToken::new();
    let child = shutdown.clone();
    let join = tokio::spawn(async move {
        let _ = axum::serve(listener, router)
            .with_graceful_shutdown(async move { child.cancelled().await })
            .await;
    });

    Ok(WebHandle {
        shutdown,
        join,
        bound,
    })
}

fn build_router(ctx: Ctx) -> Router {
    let api = Router::new()
        .route("/state", get(api_state))
        .route("/connect", post(api_connect))
        .route("/disconnect", post(api_disconnect))
        .route("/home", get(api_home))
        .route("/fs", get(api_fs))
        .route("/download", get(api_download))
        .route("/upload", post(api_upload))
        .route("/forwards", get(api_forwards))
        .route("/forward", post(api_forward))
        // Uploads may be large; don't cap the body.
        .layer(DefaultBodyLimit::disable())
        // Everything under /api requires the token.
        .route_layer(from_fn_with_state(ctx.clone(), require_token))
        .with_state(ctx);

    Router::new()
        .route("/", get(panel))
        .route("/health", get(|| async { "ok" }))
        .nest("/api", api)
}

/// The link to hand out: opening it loads the panel, whose JS reads the token
/// from the URL fragment (which is never sent to the server).
pub fn link(cfg: &WebConfig) -> String {
    let host = if cfg.lan {
        local_ip()
            .map(|ip| ip.to_string())
            .unwrap_or_else(|| "127.0.0.1".to_string())
    } else {
        "127.0.0.1".to_string()
    };
    format!("http://{host}:{}/#t={}", cfg.port, cfg.token)
}

/// Best-effort LAN IP discovery (no packets are actually sent — connecting a UDP
/// socket just picks the outbound interface).
fn local_ip() -> Option<IpAddr> {
    let sock = std::net::UdpSocket::bind("0.0.0.0:0").ok()?;
    sock.connect("8.8.8.8:80").ok()?;
    sock.local_addr().ok().map(|a| a.ip())
}

// ---------- auth ----------

async fn require_token(State(ctx): State<Ctx>, req: Request, next: Next) -> Response {
    if token_ok(&ctx.token, &req) {
        next.run(req).await
    } else {
        (
            StatusCode::UNAUTHORIZED,
            Json(json!({ "error": "unauthorized" })),
        )
            .into_response()
    }
}

fn token_ok(expected: &str, req: &Request) -> bool {
    if expected.is_empty() {
        return false;
    }
    if let Some(h) = req
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
    {
        if let Some(tok) = h.strip_prefix("Bearer ") {
            if ct_eq(tok, expected) {
                return true;
            }
        }
    }
    if let Some(q) = req.uri().query() {
        for pair in q.split('&') {
            if let Some(v) = pair.strip_prefix("t=") {
                if ct_eq(v, expected) {
                    return true;
                }
            }
        }
    }
    false
}

/// Length-independent, short-circuit-free comparison.
fn ct_eq(a: &str, b: &str) -> bool {
    let (a, b) = (a.as_bytes(), b.as_bytes());
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for (x, y) in a.iter().zip(b.iter()) {
        diff |= x ^ y;
    }
    diff == 0
}

// ---------- error mapping ----------

struct ApiError(StatusCode, String);

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (self.0, Json(json!({ "error": self.1 }))).into_response()
    }
}

impl From<NettleError> for ApiError {
    fn from(e: NettleError) -> Self {
        let code = match e {
            NettleError::NotConnected => StatusCode::CONFLICT,
            NettleError::Permission(_) => StatusCode::FORBIDDEN,
            _ => StatusCode::INTERNAL_SERVER_ERROR,
        };
        ApiError(code, e.to_string())
    }
}

// ---------- handlers ----------

#[derive(Deserialize)]
struct HostBody {
    host_id: Uuid,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct HostQuery {
    host_id: Uuid,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct FsQuery {
    host_id: Uuid,
    #[serde(default)]
    path: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ForwardBody {
    host_id: Uuid,
    port: u16,
    enabled: bool,
    #[serde(default)]
    pinned: bool,
    #[serde(default)]
    local_port: Option<u16>,
}

async fn api_state(State(ctx): State<Ctx>) -> Result<Json<Value>, ApiError> {
    let hosts = ctx.app.store.load_hosts().await;
    let sessions: Vec<Value> = ctx
        .app
        .ui
        .conn_states
        .lock()
        .unwrap()
        .iter()
        .map(|(id, conn)| json!({ "hostId": id, "conn": conn }))
        .collect();
    Ok(Json(json!({ "hosts": hosts, "sessions": sessions })))
}

async fn api_connect(
    State(ctx): State<Ctx>,
    Json(body): Json<HostBody>,
) -> Result<StatusCode, ApiError> {
    connect_host(&ctx.app, body.host_id).await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn api_disconnect(
    State(ctx): State<Ctx>,
    Json(body): Json<HostBody>,
) -> Result<StatusCode, ApiError> {
    disconnect_host(&ctx.app, body.host_id).await;
    Ok(StatusCode::NO_CONTENT)
}

async fn api_home(
    State(ctx): State<Ctx>,
    Query(q): Query<HostQuery>,
) -> Result<Json<Value>, ApiError> {
    let session = with_session(&ctx.app, q.host_id).await?;
    let home = session.browser.home().await?;
    Ok(Json(json!({ "path": home })))
}

async fn api_fs(
    State(ctx): State<Ctx>,
    Query(q): Query<FsQuery>,
) -> Result<Json<crate::ipc::types::DirListing>, ApiError> {
    let session = with_session(&ctx.app, q.host_id).await?;
    let path = if q.path.is_empty() { "~" } else { &q.path };
    Ok(Json(session.browser.list(path).await?))
}

async fn api_download(
    State(ctx): State<Ctx>,
    Query(q): Query<FsQuery>,
) -> Result<Response, ApiError> {
    if q.path.is_empty() {
        return Err(ApiError(StatusCode::BAD_REQUEST, "missing path".into()));
    }
    let session = with_session(&ctx.app, q.host_id).await?;
    let data = session.browser.read_file(&q.path).await?;
    let filename = q.path.rsplit('/').next().unwrap_or("download");
    let disposition = format!("attachment; filename=\"{}\"", filename.replace('"', ""));
    Ok((
        [
            (header::CONTENT_TYPE, "application/octet-stream".to_string()),
            (header::CONTENT_DISPOSITION, disposition),
        ],
        Body::from(data),
    )
        .into_response())
}

async fn api_upload(
    State(ctx): State<Ctx>,
    Query(q): Query<FsQuery>,
    body: Bytes,
) -> Result<StatusCode, ApiError> {
    if q.path.is_empty() {
        return Err(ApiError(StatusCode::BAD_REQUEST, "missing path".into()));
    }
    let session = with_session(&ctx.app, q.host_id).await?;
    session.browser.write_file(&q.path, &body).await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn api_forwards(
    State(ctx): State<Ctx>,
    Query(q): Query<HostQuery>,
) -> Result<Json<Vec<ForwardInfo>>, ApiError> {
    let session = with_session(&ctx.app, q.host_id).await?;
    Ok(Json(session.forwards.list()))
}

async fn api_forward(
    State(ctx): State<Ctx>,
    Json(body): Json<ForwardBody>,
) -> Result<StatusCode, ApiError> {
    let session = with_session(&ctx.app, body.host_id).await?;
    session
        .forwards
        .set_with_local(
            body.port,
            body.local_port.unwrap_or(0),
            body.enabled,
            body.pinned,
        )
        .await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn panel() -> Html<&'static str> {
    Html(PANEL_HTML)
}

const PANEL_HTML: &str = include_str!("web_panel.html");

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ct_eq_matches_only_identical_strings() {
        assert!(ct_eq("abc123", "abc123"));
        assert!(!ct_eq("abc123", "abc124"));
        assert!(!ct_eq("abc", "abcd"));
        // ct_eq is a pure byte-equality check; the empty-token guard lives in
        // token_ok, not here.
        assert!(ct_eq("", ""));
        assert!(!ct_eq("", "anything"));
    }

    #[test]
    fn localhost_link_uses_loopback_and_fragment_token() {
        let cfg = WebConfig {
            enabled: true,
            port: 8760,
            lan: false,
            token: "deadbeef".into(),
        };
        assert_eq!(link(&cfg), "http://127.0.0.1:8760/#t=deadbeef");
    }

    #[test]
    fn lan_link_embeds_the_port_and_token() {
        let cfg = WebConfig {
            enabled: true,
            port: 9000,
            lan: true,
            token: "cafe".into(),
        };
        let l = link(&cfg);
        assert!(l.ends_with(":9000/#t=cafe"), "got {l}");
        assert!(l.starts_with("http://"));
    }
}

#[cfg(test)]
mod server_tests {
    use super::*;
    use axum::body::Body;
    use axum::http::Request;
    use tower::ServiceExt; // for `oneshot`

    use crate::config::ConfigStore;
    use crate::state::{EventSink, UiBridge};

    struct NoopSink;
    impl EventSink for NoopSink {
        fn emit_json(&self, _: &str, _: serde_json::Value) {}
    }

    fn test_ctx(token: &str) -> (Ctx, tempfile::TempDir) {
        let tmp = tempfile::tempdir().unwrap();
        let store = ConfigStore::new(tmp.path().to_path_buf());
        let ui = UiBridge::new(Box::new(NoopSink));
        let app = AppState::new(store, ui);
        (
            Ctx {
                app,
                token: token.to_string(),
            },
            tmp,
        )
    }

    async fn status_of(token: &str, req: Request<Body>) -> StatusCode {
        let (ctx, _tmp) = test_ctx(token);
        build_router(ctx).oneshot(req).await.unwrap().status()
    }

    #[tokio::test]
    async fn health_and_panel_need_no_token() {
        assert_eq!(
            status_of(
                "secret",
                Request::get("/health").body(Body::empty()).unwrap()
            )
            .await,
            StatusCode::OK
        );
        assert_eq!(
            status_of("secret", Request::get("/").body(Body::empty()).unwrap()).await,
            StatusCode::OK
        );
    }

    #[tokio::test]
    async fn api_rejects_missing_and_wrong_token() {
        assert_eq!(
            status_of(
                "secret",
                Request::get("/api/state").body(Body::empty()).unwrap()
            )
            .await,
            StatusCode::UNAUTHORIZED
        );
        let wrong = Request::get("/api/state")
            .header("authorization", "Bearer nope")
            .body(Body::empty())
            .unwrap();
        assert_eq!(status_of("secret", wrong).await, StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn api_accepts_token_via_header_or_query() {
        let with_header = Request::get("/api/state")
            .header("authorization", "Bearer secret")
            .body(Body::empty())
            .unwrap();
        assert_eq!(status_of("secret", with_header).await, StatusCode::OK);

        let with_query = Request::get("/api/state?t=secret")
            .body(Body::empty())
            .unwrap();
        assert_eq!(status_of("secret", with_query).await, StatusCode::OK);
    }

    #[tokio::test]
    async fn file_ops_on_a_disconnected_host_conflict_not_crash() {
        // Hitting a host with no live session returns 409, not a panic/500.
        let host = Uuid::new_v4();
        let req = Request::get(format!("/api/fs?hostId={host}&path=~"))
            .header("authorization", "Bearer secret")
            .body(Body::empty())
            .unwrap();
        assert_eq!(status_of("secret", req).await, StatusCode::CONFLICT);
    }

    async fn raw_get(port: u16, path: &str) -> String {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let mut s = tokio::net::TcpStream::connect(("127.0.0.1", port))
            .await
            .unwrap();
        let req = format!("GET {path} HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n");
        s.write_all(req.as_bytes()).await.unwrap();
        let mut buf = Vec::new();
        s.read_to_end(&mut buf).await.unwrap();
        String::from_utf8_lossy(&buf).into_owned()
    }

    #[tokio::test]
    async fn real_socket_serves_panel_and_enforces_auth() {
        let (ctx, _tmp) = test_ctx("secret");
        let cfg = WebConfig {
            enabled: true,
            port: 0, // ephemeral — avoids clashing with anything
            lan: false,
            token: "secret".into(),
        };
        let handle = start(ctx.app.clone(), &cfg).await.unwrap();
        let port = handle.bound.port();

        assert!(raw_get(port, "/health").await.starts_with("HTTP/1.1 200"));

        let panel = raw_get(port, "/").await;
        assert!(panel.starts_with("HTTP/1.1 200"), "{panel}");
        assert!(panel.contains("nettle remote"), "panel html served");

        assert!(
            raw_get(port, "/api/state")
                .await
                .starts_with("HTTP/1.1 401"),
            "unauthenticated api must be rejected over the wire"
        );
        assert!(
            raw_get(port, "/api/state?t=secret")
                .await
                .starts_with("HTTP/1.1 200"),
            "token in query authorizes the request"
        );

        // stop() awaits the serve task, so the listener is dropped by the time
        // it returns — no dangling server.
        handle.stop().await;
    }
}
