use crate::context::{render_scene_context_markdown, SceneContextRequest};
use crate::engine::CampaignEngine;
use crate::maintenance::diagnose_campaign;
use crate::mcp::{
    all_tool_definitions, execute_tool, execute_tool_with_policy, export_tool_schemas,
    handle_mcp_jsonrpc_message_with_policy,
};
use crate::oauth::OAuthStore;
use crate::search::{
    AttachmentSearchFilter, CharacterSearchFilter, EventSearchFilter, HistorySearchFilter,
    ItemSearchFilter, LoreSearchFilter, PlaceSearchFilter, RelationSearchFilter, TextSearchFilter,
};
use anyhow::{anyhow, Context, Result};
use rand::Rng;
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

const INDEX_HTML: &str = include_str!("ui.html");

#[derive(Debug, Clone)]
pub struct ServerOptions {
    pub host: String,
    pub port: u16,
    /// Optional static Bearer token to protect `/mcp` and `/api/*` when exposed via HTTPS tunnel.
    pub bearer_token: Option<String>,
    /// Optional public origin URL (e.g. https://my-tunnel.trycloudflare.com) for OAuth metadata & ChatGPT `search`/`fetch` URLs.
    pub public_url: Option<String>,
    /// Enable built-in OAuth 2.1 + PKCE + Dynamic Client Registration for remote web-hosted MCP clients (ChatGPT/Claude/Antigravity).
    pub oauth_enabled: bool,
    /// Passphrase required on `/oauth/authorize` consent page (auto-generated if omitted while `oauth_enabled` is true).
    pub oauth_secret: Option<String>,
    /// Allow `import_attachment(source_path)` to read arbitrary local host files over HTTP (disabled by default for safety).
    pub allow_local_file_import: bool,
}

/// Starts the built-in Chronicle GM Studio Web GUI + REST / OpenAPI 3.1 + MCP Streamable HTTP server (with optional OAuth 2.1).
pub fn run_web_server(campaign_dir: &Path, opts: ServerOptions) -> Result<()> {
    let engine = CampaignEngine::open(campaign_dir)?;
    // Ensure docs exist on boot if campaign is already initialized
    if engine.db().is_initialized()? {
        let _ = engine.compile_docs();
    }
    let addr = format!("{}:{}", opts.host, opts.port);
    let listener = TcpListener::bind(&addr)
        .with_context(|| format!("Failed to bind Chronicle GM Studio on http://{addr}"))?;

    let public_base = opts
        .public_url
        .clone()
        .unwrap_or_else(|| format!("http://{addr}"));

    let oauth_store: Option<Arc<Mutex<OAuthStore>>> = if opts.oauth_enabled {
        let secret = opts
            .oauth_secret
            .clone()
            .filter(|s| !s.trim().is_empty())
            .unwrap_or_else(|| OAuthStore::generate_random_secret("gm-"));
        println!("Chronicle GM Studio & Hosted MCP Server running at: http://{addr}");
        println!("  Campaign       : {}", campaign_dir.display());
        println!("  Web GM Studio  : http://{addr}/");
        println!("  MCP Streamable : {}/mcp", public_base.trim_end_matches('/'));
        println!("  OAuth 2.1 Meta : {}/.well-known/oauth-protected-resource", public_base.trim_end_matches('/'));
        println!("  OAuth Secret   : {secret}  (enter on /oauth/authorize consent screen)");
        if opts.bearer_token.is_some() {
            println!("  Static Bearer  : enabled (also accepted alongside OAuth tokens)");
        }
        println!("  OpenAPI 3.1    : http://{addr}/api/openapi.json");
        println!("Press Ctrl+C to stop.");
        Some(Arc::new(Mutex::new(OAuthStore::new(secret))))
    } else {
        println!("Chronicle GM Studio & MCP Server running at: http://{addr}");
        println!("  Campaign       : {}", campaign_dir.display());
        println!("  Web GM Studio  : http://{addr}/");
        println!("  MCP Streamable : http://{addr}/mcp  (ChatGPT Web/Desktop, Antigravity & Codex CLI)");
        println!("  REST API       : http://{addr}/api/status");
        println!("  OpenAPI 3.1    : http://{addr}/api/openapi.json");
        if opts.bearer_token.is_some() {
            println!("  Auth           : Static Bearer token enabled");
        }
        println!("Press Ctrl+C to stop.");
        None
    };

    let shared = Arc::new(Mutex::new(engine));
    let root_path = campaign_dir.to_path_buf();
    let bearer = Arc::new(opts.bearer_token);
    let explicit_public = Arc::new(opts.public_url);
    let fallback_base = Arc::new(public_base);
    let allow_local_file_import = opts.allow_local_file_import;
    if !allow_local_file_import {
        println!("  HTTP File Guard: import_attachment(source_path) disabled over HTTP (use inline text/base64 or --allow-local-file-import)");
    }

    // Best-effort IPv6 loopback listener when bound to 127.0.0.1 / localhost so reverse proxies
    // like `cloudflared` configured with `http://localhost:<port>` work even if `localhost`
    // resolves to IPv6 `::1` first (common on Linux / Google Colab).
    if opts.host == "127.0.0.1" || opts.host.eq_ignore_ascii_case("localhost") {
        let ipv6_addr = format!("[::1]:{}", opts.port);
        if let Ok(v6_listener) = TcpListener::bind(&ipv6_addr) {
            let eng6 = Arc::clone(&shared);
            let root6 = root_path.clone();
            let tok6 = Arc::clone(&bearer);
            let pub6 = Arc::clone(&explicit_public);
            let fb6 = Arc::clone(&fallback_base);
            let oa6 = oauth_store.clone();
            std::thread::spawn(move || {
                for stream_res in v6_listener.incoming() {
                    let stream = match stream_res {
                        Ok(s) => s,
                        Err(_) => continue,
                    };
                    let eng = Arc::clone(&eng6);
                    let root = root6.clone();
                    let tok = Arc::clone(&tok6);
                    let pub_opt = Arc::clone(&pub6);
                    let fb_base = Arc::clone(&fb6);
                    let oa = oa6.clone();
                    std::thread::spawn(move || {
                        if let Err(e) = handle_connection(
                            stream,
                            eng,
                            &root,
                            tok.as_deref(),
                            pub_opt.as_deref(),
                            fb_base.as_str(),
                            oa,
                            allow_local_file_import,
                        ) {
                            eprintln!("[chronicle-ui] request error: {e}");
                        }
                    });
                }
            });
        }
    }

    for stream_res in listener.incoming() {
        let stream = match stream_res {
            Ok(s) => s,
            Err(e) => {
                eprintln!("[chronicle-ui] accept error: {e}");
                continue;
            }
        };
        let eng = Arc::clone(&shared);
        let root = root_path.clone();
        let tok = Arc::clone(&bearer);
        let pub_opt = Arc::clone(&explicit_public);
        let fb_base = Arc::clone(&fallback_base);
        let oa = oauth_store.clone();
        std::thread::spawn(move || {
            if let Err(e) = handle_connection(
                stream,
                eng,
                &root,
                tok.as_deref(),
                pub_opt.as_deref(),
                fb_base.as_str(),
                oa,
                allow_local_file_import,
            ) {
                eprintln!("[chronicle-ui] request error: {e}");
            }
        });
    }

    Ok(())
}

struct HttpRequest {
    method: String,
    path: String,
    query: BTreeMap<String, String>,
    headers: BTreeMap<String, String>,
    body: Vec<u8>,
}

struct HttpOutcome {
    status: u16,
    status_text: &'static str,
    content_type: String,
    body: Vec<u8>,
    extra_headers: Vec<(String, String)>,
}

fn infer_request_origin(
    req: &HttpRequest,
    explicit_public_url: Option<&str>,
    fallback_base: &str,
) -> String {
    if let Some(p) = explicit_public_url {
        if !p.trim().is_empty() {
            return p.trim_end_matches('/').to_string();
        }
    }
    if let Some(host) = req.headers.get("host") {
        let proto = req
            .headers
            .get("x-forwarded-proto")
            .map(|s| s.split(',').next().unwrap_or("https").trim())
            .unwrap_or_else(|| {
                if host.starts_with("127.0.0.1") || host.starts_with("localhost") {
                    "http"
                } else {
                    "https"
                }
            });
        return format!("{proto}://{host}");
    }
    fallback_base.trim_end_matches('/').to_string()
}

fn is_mcp_path(path: &str) -> bool {
    path == "/mcp" || path == "/mcp/"
}

fn handle_oauth_endpoint(
    req: &HttpRequest,
    origin: &str,
    oauth: Option<&Arc<Mutex<OAuthStore>>>,
) -> Option<HttpOutcome> {
    let path = req.path.as_str();
    if (req.method == "GET" || req.method == "HEAD") && path == "/health" {
        let body = if req.method == "HEAD" {
            vec![]
        } else {
            json!({ "status": "ok", "oauth_enabled": oauth.is_some() })
                .to_string()
                .into_bytes()
        };
        return Some(HttpOutcome {
            status: 200,
            status_text: "OK",
            content_type: "application/json; charset=utf-8".to_string(),
            body,
            extra_headers: vec![],
        });
    }

    let store_arc = oauth?;
    if req.method == "GET"
        && (path == "/.well-known/oauth-protected-resource"
            || path == "/.well-known/oauth-protected-resource/mcp"
            || path == "/.well-known/oauth-protected-resource/mcp/")
    {
        let meta = OAuthStore::protected_resource_metadata(origin);
        return Some(HttpOutcome {
            status: 200,
            status_text: "OK",
            content_type: "application/json; charset=utf-8".to_string(),
            body: serde_json::to_vec(&meta).unwrap_or_default(),
            extra_headers: vec![],
        });
    }
    if req.method == "GET"
        && (path == "/.well-known/oauth-authorization-server"
            || path == "/.well-known/oauth-authorization-server/mcp"
            || path == "/mcp/.well-known/oauth-authorization-server"
            || path == "/.well-known/openid-configuration"
            || path == "/.well-known/openid-configuration/mcp"
            || path == "/mcp/.well-known/openid-configuration")
    {
        let meta = OAuthStore::authorization_server_metadata(origin);
        return Some(HttpOutcome {
            status: 200,
            status_text: "OK",
            content_type: "application/json; charset=utf-8".to_string(),
            body: serde_json::to_vec(&meta).unwrap_or_default(),
            extra_headers: vec![],
        });
    }
    if req.method == "POST" && path == "/oauth/register" {
        let mut store = store_arc.lock().ok()?;
        let r = store.handle_register(&req.body);
        return Some(HttpOutcome {
            status: r.status,
            status_text: r.status_text,
            content_type: r.content_type,
            body: r.body,
            extra_headers: r.extra_headers,
        });
    }
    if req.method == "GET" && path == "/oauth/authorize" {
        let store = store_arc.lock().ok()?;
        let r = store.handle_authorize_get(&req.query);
        return Some(HttpOutcome {
            status: r.status,
            status_text: r.status_text,
            content_type: r.content_type,
            body: r.body,
            extra_headers: r.extra_headers,
        });
    }
    if req.method == "POST" && path == "/oauth/authorize" {
        let mut store = store_arc.lock().ok()?;
        let r = store.handle_authorize_post(&req.body);
        return Some(HttpOutcome {
            status: r.status,
            status_text: r.status_text,
            content_type: r.content_type,
            body: r.body,
            extra_headers: r.extra_headers,
        });
    }
    if req.method == "POST" && path == "/oauth/token" {
        let mut store = store_arc.lock().ok()?;
        let r = store.handle_token(&req.headers, &req.body);
        return Some(HttpOutcome {
            status: r.status,
            status_text: r.status_text,
            content_type: r.content_type,
            body: r.body,
            extra_headers: r.extra_headers,
        });
    }
    None
}

fn check_authorization(
    req: &HttpRequest,
    origin: &str,
    bearer_token: Option<&str>,
    oauth: Option<&Arc<Mutex<OAuthStore>>>,
) -> Option<HttpOutcome> {
    if bearer_token.is_none() && oauth.is_none() {
        return None;
    }
    let protected = is_mcp_path(&req.path) || req.path.starts_with("/api/");
    if !protected {
        return None;
    }
    let provided = req
        .headers
        .get("authorization")
        .and_then(|s| s.trim().strip_prefix("Bearer ").or_else(|| s.trim().strip_prefix("bearer ")))
        .map(|s| s.trim())
        .unwrap_or("");

    if !provided.is_empty() {
        if let Some(expected) = bearer_token {
            if provided == expected {
                return None;
            }
        }
        if let Some(store_arc) = oauth {
            if let Ok(mut store) = store_arc.lock() {
                if store.validate_access_token(provided) {
                    return None;
                }
            }
        }
    }

    let www_auth = if oauth.is_some() {
        format!(
            "Bearer realm=\"chronicle-mcp\", resource_metadata=\"{}/.well-known/oauth-protected-resource\"",
            origin.trim_end_matches('/')
        )
    } else {
        "Bearer realm=\"chronicle-mcp\"".to_string()
    };

    Some(HttpOutcome {
        status: 401,
        status_text: "Unauthorized",
        content_type: "application/json; charset=utf-8".to_string(),
        body: json!({ "error": "Unauthorized: missing or invalid Bearer / OAuth access token" })
            .to_string()
            .into_bytes(),
        extra_headers: vec![("WWW-Authenticate".to_string(), www_auth)],
    })
}

fn handle_connection(
    mut stream: TcpStream,
    engine_mutex: Arc<Mutex<CampaignEngine>>,
    campaign_dir: &Path,
    bearer_token: Option<&str>,
    explicit_public_url: Option<&str>,
    fallback_base: &str,
    oauth: Option<Arc<Mutex<OAuthStore>>>,
    allow_local_file_import: bool,
) -> Result<()> {
    let _ = stream.set_read_timeout(Some(Duration::from_secs(30)));
    let req = match read_http_request(&mut stream)? {
        Some(r) => r,
        None => return Ok(()),
    };

    // Handle CORS preflight cleanly for external AI tools/extensions & ChatGPT MCP
    if req.method == "OPTIONS" {
        log_http_request(&req.method, &req.path, 204, 0);
        return send_response(
            &mut stream,
            204,
            "No Content",
            "text/plain; charset=utf-8",
            b"",
            &[],
        );
    }

    let origin = infer_request_origin(&req, explicit_public_url, fallback_base);

    // OAuth 2.1 discovery, dynamic client registration, consent & token endpoints (+ /health)
    if let Some(outcome) = handle_oauth_endpoint(&req, &origin, oauth.as_ref()) {
        log_http_request(&req.method, &req.path, outcome.status, outcome.body.len());
        return send_response(
            &mut stream,
            outcome.status,
            outcome.status_text,
            &outcome.content_type,
            &outcome.body,
            &outcome.extra_headers,
        );
    }

    // Enforce Bearer / OAuth access token on /mcp and /api/*
    if let Some(unauth) = check_authorization(&req, &origin, bearer_token, oauth.as_ref()) {
        log_http_request(&req.method, &req.path, unauth.status, unauth.body.len());
        return send_response(
            &mut stream,
            unauth.status,
            unauth.status_text,
            &unauth.content_type,
            &unauth.body,
            &unauth.extra_headers,
        );
    }

    let outcome = route_http_full(
        &req,
        &engine_mutex,
        campaign_dir,
        &origin,
        allow_local_file_import,
    );
    log_http_request(&req.method, &req.path, outcome.status, outcome.body.len());
    send_response(
        &mut stream,
        outcome.status,
        outcome.status_text,
        &outcome.content_type,
        &outcome.body,
        &outcome.extra_headers,
    )
}

fn log_http_request(method: &str, path: &str, status: u16, bytes: usize) {
    println!("[chronicle-http] {method} {path} -> {status} ({bytes}B)");
    let _ = std::io::stdout().flush();
}

/// Public in-process HTTP router with OAuth 2.1 + headers support for deterministic testing.
pub fn dispatch_http_with_oauth(
    engine: &CampaignEngine,
    oauth: &Arc<Mutex<OAuthStore>>,
    public_origin: &str,
    method: &str,
    target: &str,
    headers: BTreeMap<String, String>,
    body: &[u8],
) -> (u16, String, Vec<(String, String)>, Vec<u8>) {
    let (raw_path, raw_query) = match target.split_once('?') {
        Some((p, q)) => (p, Some(q)),
        None => (target, None),
    };
    let mut query = BTreeMap::new();
    if let Some(q) = raw_query {
        for pair in q.split('&') {
            if pair.is_empty() {
                continue;
            }
            if let Some((k, v)) = pair.split_once('=') {
                query.insert(url_decode(k), url_decode(v));
            } else {
                query.insert(url_decode(pair), String::new());
            }
        }
    }
    let req = HttpRequest {
        method: method.to_uppercase(),
        path: raw_path.to_string(),
        query,
        headers,
        body: body.to_vec(),
    };
    if let Some(out) = handle_oauth_endpoint(&req, public_origin, Some(oauth)) {
        return (out.status, out.content_type, out.extra_headers, out.body);
    }
    if let Some(unauth) = check_authorization(&req, public_origin, None, Some(oauth)) {
        return (
            unauth.status,
            unauth.content_type,
            unauth.extra_headers,
            unauth.body,
        );
    }
    if is_mcp_path(&req.path) {
        let out = handle_mcp_http(&req, engine, public_origin, false);
        return (out.status, out.content_type, out.extra_headers, out.body);
    }
    match route_request_with_engine(&req, engine, engine.root_dir(), false) {
        Ok((ct, bytes)) => (200, ct, vec![], bytes),
        Err(e) => (
            400,
            "application/json; charset=utf-8".to_string(),
            vec![],
            json!({ "error": e.to_string() }).to_string().into_bytes(),
        ),
    }
}

/// Public in-process HTTP router for deterministic testing and embedding without requiring a socket.
/// Supports all Web GUI, REST, OpenAPI, AND Streamable HTTP MCP (`/mcp`) endpoints.
pub fn dispatch_http(
    engine: &CampaignEngine,
    method: &str,
    target: &str,
    body: &[u8],
) -> (u16, String, Vec<u8>) {
    let (raw_path, raw_query) = match target.split_once('?') {
        Some((p, q)) => (p, Some(q)),
        None => (target, None),
    };
    let mut query = BTreeMap::new();
    if let Some(q) = raw_query {
        for pair in q.split('&') {
            if pair.is_empty() {
                continue;
            }
            if let Some((k, v)) = pair.split_once('=') {
                query.insert(url_decode(k), url_decode(v));
            } else {
                query.insert(url_decode(pair), String::new());
            }
        }
    }
    let req = HttpRequest {
        method: method.to_uppercase(),
        path: raw_path.to_string(),
        query,
        headers: BTreeMap::new(),
        body: body.to_vec(),
    };
    if req.method == "OPTIONS" {
        return (204, "text/plain; charset=utf-8".to_string(), Vec::new());
    }
    if is_mcp_path(&req.path) {
        let out = handle_mcp_http(&req, engine, "http://127.0.0.1:7878", false);
        return (out.status, out.content_type, out.body);
    }
    match route_request_with_engine(&req, engine, engine.root_dir(), false) {
        Ok((ct, bytes)) => (200, ct, bytes),
        Err(e) => (
            400,
            "application/json; charset=utf-8".to_string(),
            json!({ "error": e.to_string() }).to_string().into_bytes(),
        ),
    }
}

fn route_http_full(
    req: &HttpRequest,
    engine_mutex: &Arc<Mutex<CampaignEngine>>,
    campaign_dir: &Path,
    base_url: &str,
    allow_local_file_import: bool,
) -> HttpOutcome {
    if is_mcp_path(&req.path) {
        let Ok(engine) = engine_mutex.lock() else {
            return HttpOutcome {
                status: 500,
                status_text: "Internal Server Error",
                content_type: "application/json; charset=utf-8".to_string(),
                body: json!({ "error": "Engine lock poisoned" }).to_string().into_bytes(),
                extra_headers: vec![],
            };
        };
        return handle_mcp_http(req, &engine, base_url, allow_local_file_import);
    }

    match route_request(req, engine_mutex, campaign_dir, allow_local_file_import) {
        Ok((ct, bytes)) => HttpOutcome {
            status: 200,
            status_text: "OK",
            content_type: ct,
            body: bytes,
            extra_headers: vec![],
        },
        Err(e) => HttpOutcome {
            status: 400,
            status_text: "Bad Request",
            content_type: "application/json; charset=utf-8".to_string(),
            body: json!({ "error": e.to_string() }).to_string().into_bytes(),
            extra_headers: vec![],
        },
    }
}

/// Implements MCP Streamable HTTP Transport (`2025-03-26` / `2025-06-18`) on `/mcp`:
/// - `POST /mcp`: JSON-RPC 2.0 single request, notification (`202 Accepted`), or batch array.
/// - `GET /mcp`: Server metadata / `405 Method Not Allowed` for stateless SSE polling.
/// - `DELETE /mcp`: Clean session teardown (`200 OK`).
fn handle_mcp_http(
    req: &HttpRequest,
    engine: &CampaignEngine,
    base_url: &str,
    allow_local_file_import: bool,
) -> HttpOutcome {
    let incoming_session = req.headers.get("mcp-session-id").cloned();

    match req.method.as_str() {
        "DELETE" => HttpOutcome {
            status: 200,
            status_text: "OK",
            content_type: "application/json; charset=utf-8".to_string(),
            body: b"{}".to_vec(),
            extra_headers: vec![],
        },
        "GET" => HttpOutcome {
            status: 405,
            status_text: "Method Not Allowed",
            content_type: "application/json; charset=utf-8".to_string(),
            body: json!({
                "error": "Use POST /mcp with JSON-RPC 2.0 payloads (MCP Streamable HTTP)."
            })
            .to_string()
            .into_bytes(),
            extra_headers: vec![("Allow".to_string(), "POST, DELETE, OPTIONS".to_string())],
        },
        "POST" => {
            let parsed: Value = match serde_json::from_slice(&req.body) {
                Ok(v) => v,
                Err(e) => {
                    return HttpOutcome {
                        status: 400,
                        status_text: "Bad Request",
                        content_type: "application/json; charset=utf-8".to_string(),
                        body: json!({
                            "jsonrpc": "2.0",
                            "id": Value::Null,
                            "error": { "code": -32700, "message": format!("Parse error: {e}") }
                        })
                        .to_string()
                        .into_bytes(),
                        extra_headers: vec![],
                    };
                }
            };

            let session_id = incoming_session.unwrap_or_else(generate_session_id);
            let session_hdr = vec![("Mcp-Session-Id".to_string(), session_id)];
            let wants_sse_only = req
                .headers
                .get("accept")
                .map(|a| {
                    let lower = a.to_lowercase();
                    lower.contains("text/event-stream")
                        && !lower.contains("application/json")
                        && !lower.contains("*/*")
                })
                .unwrap_or(false);

            let make_mcp_ok = |payload: Value, hdrs: Vec<(String, String)>| {
                let json_bytes = serde_json::to_vec(&payload).unwrap_or_default();
                if wants_sse_only {
                    let mut sse = Vec::with_capacity(json_bytes.len() + 32);
                    sse.extend_from_slice(b"event: message\ndata: ");
                    sse.extend_from_slice(&json_bytes);
                    sse.extend_from_slice(b"\n\n");
                    HttpOutcome {
                        status: 200,
                        status_text: "OK",
                        content_type: "text/event-stream; charset=utf-8".to_string(),
                        body: sse,
                        extra_headers: hdrs,
                    }
                } else {
                    HttpOutcome {
                        status: 200,
                        status_text: "OK",
                        content_type: "application/json; charset=utf-8".to_string(),
                        body: json_bytes,
                        extra_headers: hdrs,
                    }
                }
            };

            // Support JSON-RPC 2.0 batch arrays as well as single objects
            if let Value::Array(items) = parsed {
                let mut responses = Vec::new();
                for item in items {
                    if let Some(resp) = handle_mcp_jsonrpc_message_with_policy(
                        engine,
                        item,
                        Some(base_url),
                        allow_local_file_import,
                    ) {
                        responses.push(resp);
                    }
                }
                if responses.is_empty() {
                    return HttpOutcome {
                        status: 202,
                        status_text: "Accepted",
                        content_type: "application/json; charset=utf-8".to_string(),
                        body: vec![],
                        extra_headers: session_hdr,
                    };
                }
                return make_mcp_ok(Value::Array(responses), session_hdr);
            }

            match handle_mcp_jsonrpc_message_with_policy(
                engine,
                parsed,
                Some(base_url),
                allow_local_file_import,
            ) {
                Some(resp) => make_mcp_ok(resp, session_hdr),
                None => HttpOutcome {
                    status: 202,
                    status_text: "Accepted",
                    content_type: "application/json; charset=utf-8".to_string(),
                    body: vec![],
                    extra_headers: session_hdr,
                },
            }
        }
        _ => HttpOutcome {
            status: 405,
            status_text: "Method Not Allowed",
            content_type: "application/json; charset=utf-8".to_string(),
            body: json!({ "error": "Method not allowed on /mcp" })
                .to_string()
                .into_bytes(),
            extra_headers: vec![("Allow".to_string(), "POST, DELETE, OPTIONS".to_string())],
        },
    }
}

fn generate_session_id() -> String {
    let mut rng = rand::thread_rng();
    let a: u64 = rng.gen();
    let b: u64 = rng.gen();
    format!("chronicle-{a:016x}{b:016x}")
}

fn route_request(
    req: &HttpRequest,
    engine_mutex: &Arc<Mutex<CampaignEngine>>,
    campaign_dir: &Path,
    allow_local_file_import: bool,
) -> Result<(String, Vec<u8>)> {
    let path = req.path.as_str();
    if req.method == "GET" && (path == "/" || path == "/index.html") {
        return Ok((
            "text/html; charset=utf-8".to_string(),
            INDEX_HTML.as_bytes().to_vec(),
        ));
    }
    if req.method == "GET" && path == "/api/openapi.json" {
        let spec = build_openapi_spec();
        return Ok((
            "application/json; charset=utf-8".to_string(),
            serde_json::to_vec_pretty(&spec)?,
        ));
    }
    if req.method == "GET" && path == "/api/tools" {
        let fmt = req
            .query
            .get("format")
            .map(|s| s.as_str())
            .unwrap_or("mcp");
        let schemas = export_tool_schemas(fmt)?;
        return Ok((
            "application/json; charset=utf-8".to_string(),
            serde_json::to_vec_pretty(&schemas)?,
        ));
    }
    let engine = engine_mutex
        .lock()
        .map_err(|_| anyhow!("Engine lock poisoned"))?;
    route_request_with_engine(req, &engine, campaign_dir, allow_local_file_import)
}

fn route_request_with_engine(
    req: &HttpRequest,
    engine: &CampaignEngine,
    campaign_dir: &Path,
    allow_local_file_import: bool,
) -> Result<(String, Vec<u8>)> {
    let path = req.path.as_str();

    // Static embedded SPA
    if req.method == "GET" && (path == "/" || path == "/index.html") {
        return Ok((
            "text/html; charset=utf-8".to_string(),
            INDEX_HTML.as_bytes().to_vec(),
        ));
    }

    if req.method == "GET" && path == "/api/openapi.json" {
        let spec = build_openapi_spec();
        return Ok((
            "application/json; charset=utf-8".to_string(),
            serde_json::to_vec_pretty(&spec)?,
        ));
    }

    if req.method == "GET" && path == "/api/tools" {
        let fmt = req
            .query
            .get("format")
            .map(|s| s.as_str())
            .unwrap_or("mcp");
        let schemas = export_tool_schemas(fmt)?;
        return Ok((
            "application/json; charset=utf-8".to_string(),
            serde_json::to_vec_pretty(&schemas)?,
        ));
    }

    if req.method == "GET" && path == "/api/status" {
        let data = execute_tool(&engine, "campaign_status", json!({}))?;
        return json_bytes(&data);
    }

    if req.method == "GET" && path == "/api/diagnostics" {
        let report = diagnose_campaign(engine)?;
        return json_bytes(&serde_json::to_value(report)?);
    }

    if req.method == "GET" && path == "/api/search" {
        let q = req.query.get("q").or_else(|| req.query.get("query")).cloned().unwrap_or_default();
        let limit = parse_opt_usize(&req.query, "limit").unwrap_or(15);
        let res = engine.search_omni(&q, limit)?;
        return json_bytes(&serde_json::to_value(res)?);
    }

    if req.method == "GET" && path == "/api/characters" {
        let filter = CharacterSearchFilter {
            query: opt_q(&req.query, "query"),
            kind: opt_q(&req.query, "kind"),
            alive_status: opt_q(&req.query, "alive_status"),
            location_id: opt_q(&req.query, "location_id"),
            faction: opt_q(&req.query, "faction"),
            has_item: opt_q(&req.query, "has_item"),
            equipped_only: parse_opt_bool(&req.query, "equipped_only"),
            has_skill: opt_q(&req.query, "has_skill"),
            has_technique: opt_q(&req.query, "has_technique"),
            has_condition: opt_q(&req.query, "has_condition"),
            tag: opt_q(&req.query, "tag"),
            stat_name: opt_q(&req.query, "stat_name"),
            stat_min: parse_opt_i64(&req.query, "stat_min"),
            resource_name: opt_q(&req.query, "resource_name"),
            resource_below_pct: parse_opt_u8(&req.query, "resource_below_pct"),
        };
        let list = engine.search_characters_by(&filter)?;
        return json_bytes(&json!({ "count": list.len(), "characters": list }));
    }

    if req.method == "GET" && path.starts_with("/api/characters/") {
        let id = url_decode(&path["/api/characters/".len()..]);
        let as_of = parse_opt_i64(&req.query, "as_of_seq");
        let proj = engine.get_character(&id, as_of)?;
        let rels = engine.db().list_relations_for_entity(&proj.baseline.id)?;
        let histories = engine.db().list_histories_for_entity(&proj.baseline.id)?;
        return json_bytes(&json!({
            "character": proj,
            "relations": rels,
            "histories": histories,
            "doc_path": format!("docs/characters/{}.md", proj.baseline.id)
        }));
    }

    if req.method == "GET" && path == "/api/places" {
        let filter = PlaceSearchFilter {
            query: opt_q(&req.query, "query"),
            place_type: opt_q(&req.query, "place_type"),
            parent_place: opt_q(&req.query, "parent_place"),
            terrain: opt_q(&req.query, "terrain"),
            climate: opt_q(&req.query, "climate"),
            season_effect: opt_q(&req.query, "season_effect"),
            weather: opt_q(&req.query, "weather"),
            faction: opt_q(&req.query, "faction"),
            danger_level: opt_q(&req.query, "danger_level"),
            has_landmark: opt_q(&req.query, "has_landmark"),
            landmark_status: opt_q(&req.query, "landmark_status"),
            has_condition: opt_q(&req.query, "has_condition"),
            has_character: opt_q(&req.query, "has_character"),
            connected_to: opt_q(&req.query, "connected_to"),
            tag: opt_q(&req.query, "tag"),
        };
        let list = engine.search_places_by(&filter)?;
        return json_bytes(&json!({ "count": list.len(), "places": list }));
    }

    if req.method == "GET" && path.starts_with("/api/places/") {
        let id = url_decode(&path["/api/places/".len()..]);
        let as_of = parse_opt_i64(&req.query, "as_of_seq");
        let proj = engine.get_place(&id, as_of)?;
        let rels = engine.db().list_relations_for_entity(&proj.baseline.id)?;
        let histories = engine.db().list_histories_for_entity(&proj.baseline.id)?;
        return json_bytes(&json!({
            "place": proj,
            "relations": rels,
            "histories": histories,
            "doc_path": format!("docs/places/{}.md", proj.baseline.id)
        }));
    }

    if req.method == "GET" && path == "/api/lore" {
        let filter = LoreSearchFilter {
            query: opt_q(&req.query, "query"),
            category: opt_q(&req.query, "category"),
            visibility: opt_q(&req.query, "visibility"),
            keyword: opt_q(&req.query, "keyword"),
            tag: opt_q(&req.query, "tag"),
            related_to: opt_q(&req.query, "related_to"),
            always_include: parse_opt_bool(&req.query, "always_include"),
            limit: parse_opt_usize(&req.query, "limit"),
        };
        let list = engine.search_lore_by(&filter)?;
        return json_bytes(&json!({ "count": list.len(), "entries": list }));
    }

    if req.method == "GET" && path == "/api/texts" {
        let filter = TextSearchFilter {
            query: opt_q(&req.query, "query"),
            medium: opt_q(&req.query, "medium"),
            subject: opt_q(&req.query, "subject"),
            author: opt_q(&req.query, "author"),
            language: opt_q(&req.query, "language"),
            condition: opt_q(&req.query, "condition"),
            read_status: opt_q(&req.query, "read_status"),
            read_by: opt_q(&req.query, "read_by"),
            holder_or_location_id: opt_q(&req.query, "holder_or_location_id"),
            keyword: opt_q(&req.query, "keyword"),
            tag: opt_q(&req.query, "tag"),
            related_to: opt_q(&req.query, "related_to"),
            limit: parse_opt_usize(&req.query, "limit"),
        };
        let list = engine.search_texts_by(&filter)?;
        return json_bytes(&json!({ "count": list.len(), "texts": list }));
    }

    if req.method == "GET" && path.starts_with("/api/texts/") {
        let id = url_decode(&path["/api/texts/".len()..]);
        let text = engine.get_text(&id)?;
        let events = engine.db().list_events_for_entity(&text.id)?;
        let rels = engine.db().list_relations_for_entity(&text.id)?;
        let histories = engine.db().list_histories_for_entity(&text.id)?;
        return json_bytes(&json!({
            "text": text,
            "relations": rels,
            "histories": histories,
            "event_log": events,
            "doc_path": format!("docs/texts/{}.md", text.id)
        }));
    }

    if req.method == "GET" && path == "/api/items" {
        let filter = ItemSearchFilter {
            query: opt_q(&req.query, "query"),
            item_type: opt_q(&req.query, "item_type"),
            rarity_or_tier: opt_q(&req.query, "rarity_or_tier"),
            current_holder_id: opt_q(&req.query, "current_holder_id"),
            previous_owner: opt_q(&req.query, "previous_owner"),
            condition: opt_q(&req.query, "condition"),
            has_linked_text: parse_opt_bool(&req.query, "has_linked_text"),
            keyword: opt_q(&req.query, "keyword"),
            tag: opt_q(&req.query, "tag"),
            related_to: opt_q(&req.query, "related_to"),
            limit: parse_opt_usize(&req.query, "limit"),
        };
        let list = engine.search_items_by(&filter)?;
        return json_bytes(&json!({ "count": list.len(), "items": list }));
    }

    if req.method == "GET" && path.starts_with("/api/items/") {
        let id = url_decode(&path["/api/items/".len()..]);
        let item = engine.get_catalog_item(&id)?;
        let histories = engine.db().list_histories_for_entity(&item.id)?;
        let rels = engine.db().list_relations_for_entity(&item.id)?;
        let events = engine.db().list_events_for_entity(&item.id)?;
        return json_bytes(&json!({
            "item": item,
            "histories": histories,
            "relations": rels,
            "event_log": events,
            "doc_path": format!("docs/items/{}.md", item.id)
        }));
    }

    if req.method == "GET" && path == "/api/histories" {
        let filter = HistorySearchFilter {
            query: opt_q(&req.query, "query"),
            entity_id: opt_q(&req.query, "entity_id"),
            entity_kind: opt_q(&req.query, "entity_kind"),
            era_or_date: opt_q(&req.query, "era_or_date"),
            visibility: opt_q(&req.query, "visibility"),
            related_to: opt_q(&req.query, "related_to"),
            limit: parse_opt_usize(&req.query, "limit"),
        };
        let list = engine.search_histories_by(&filter)?;
        return json_bytes(&json!({ "count": list.len(), "histories": list }));
    }

    if req.method == "GET" && path == "/api/attachments" {
        let filter = AttachmentSearchFilter {
            query: opt_q(&req.query, "query"),
            category: opt_q(&req.query, "category"),
            mime_type: opt_q(&req.query, "mime_type"),
            visibility: opt_q(&req.query, "visibility"),
            keyword: opt_q(&req.query, "keyword"),
            tag: opt_q(&req.query, "tag"),
            related_to: opt_q(&req.query, "related_to"),
            limit: parse_opt_usize(&req.query, "limit"),
        };
        let list = engine.search_attachments_by(&filter)?;
        return json_bytes(&json!({ "count": list.len(), "attachments": list }));
    }

    if req.method == "GET" && path.starts_with("/api/attachments/") {
        let rest = &path["/api/attachments/".len()..];
        if let Some(raw_id) = rest.strip_suffix("/file") {
            let id = url_decode(raw_id);
            let att = engine.get_attachment(&id)?;
            let file_path = engine.attachment_file_path(&att);
            let bytes = std::fs::read(&file_path)
                .with_context(|| format!("Attachment file missing: {}", file_path.display()))?;
            return Ok((att.mime_type, bytes));
        }
        let id = url_decode(rest);
        let att = engine.get_attachment(&id)?;
        let events = engine.db().list_events_for_entity(&att.id)?;
        let rels = engine.db().list_relations_for_entity(&att.id)?;
        let text_content = if att.mime_type.starts_with("text/") || att.mime_type == "application/json" {
            std::fs::read_to_string(engine.attachment_file_path(&att)).ok()
        } else {
            None
        };
        return json_bytes(&json!({
            "attachment": att,
            "text_content": text_content,
            "relations": rels,
            "event_log": events,
            "file_url": format!("/api/attachments/{}/file", att.id),
            "mcp_uri": format!("chronicle://attachments/{}", att.id)
        }));
    }

    if req.method == "GET" && path == "/api/relations" {
        let filter = RelationSearchFilter {
            entity_id: opt_q(&req.query, "entity_id"),
            source_id: opt_q(&req.query, "source_id"),
            target_id: opt_q(&req.query, "target_id"),
            relation_type: opt_q(&req.query, "relation_type"),
            min_strength: parse_opt_i64(&req.query, "min_strength"),
            max_strength: parse_opt_i64(&req.query, "max_strength"),
            query: opt_q(&req.query, "query"),
            hops: parse_opt_u8(&req.query, "hops"),
        };
        let list = engine.search_relations_by(&filter)?;
        return json_bytes(&json!({ "count": list.len(), "relations": list }));
    }

    if req.method == "GET" && path == "/api/events" {
        let filter = EventSearchFilter {
            query: opt_q(&req.query, "query"),
            entity_id: opt_q(&req.query, "entity_id"),
            location_id: opt_q(&req.query, "location_id"),
            event_type: opt_q(&req.query, "event_type"),
            session_number: parse_opt_u32(&req.query, "session_number"),
            world_time_contains: opt_q(&req.query, "world_time_contains"),
            seq_from: parse_opt_i64(&req.query, "seq_from"),
            seq_to: parse_opt_i64(&req.query, "seq_to"),
            limit: parse_opt_usize(&req.query, "limit").or(Some(100)),
        };
        let list = engine.search_events_by(&filter)?;
        return json_bytes(&json!({ "count": list.len(), "events": list }));
    }

    if req.method == "GET" && path == "/api/clocks" {
        let list = engine.db().list_clocks()?;
        return json_bytes(&json!({ "count": list.len(), "clocks": list }));
    }

    if req.method == "GET" && path == "/api/profiles" {
        let list = engine.list_ruleset_profiles()?;
        return json_bytes(&json!({ "count": list.len(), "profiles": list }));
    }

    if req.method == "GET" && path == "/api/tables" {
        let list = engine.list_random_tables()?;
        return json_bytes(&json!({ "count": list.len(), "tables": list }));
    }

    if req.method == "GET" && path == "/api/discoveries" {
        let st = opt_q(&req.query, "status");
        let list = engine.list_staged_discoveries(st.as_deref())?;
        return json_bytes(&json!({ "count": list.len(), "discoveries": list }));
    }

    if req.method == "GET" && path == "/api/encounters" {
        let st = opt_q(&req.query, "status");
        let list = engine.list_encounters(st.as_deref())?;
        return json_bytes(&json!({ "count": list.len(), "encounters": list }));
    }

    if req.method == "GET" && path == "/api/factions" {
        let list = engine.list_factions()?;
        return json_bytes(&json!({ "count": list.len(), "factions": list }));
    }

    if req.method == "GET" && path == "/api/knowledge" {
        let filter = crate::autonomy::KnowledgeSearchFilter {
            query: opt_q(&req.query, "query"),
            actor_id: opt_q(&req.query, "actor_id"),
            not_known_by: opt_q(&req.query, "not_known_by"),
            subject: opt_q(&req.query, "subject"),
            stance: opt_q(&req.query, "stance"),
            truth_status: opt_q(&req.query, "truth_status"),
            visibility: opt_q(&req.query, "visibility"),
            min_confidence: parse_opt_u8(&req.query, "min_confidence"),
            include_outdated: parse_opt_bool(&req.query, "include_outdated"),
            limit: parse_opt_usize(&req.query, "limit"),
        };
        let list = engine.search_knowledge(&filter)?;
        return json_bytes(&json!({ "count": list.len(), "records": list }));
    }

    if req.method == "GET" && path == "/api/routes" {
        let list = engine.all_effective_routes()?;
        return json_bytes(&json!({ "count": list.len(), "routes": list }));
    }

    if req.method == "GET" && path == "/api/markets" {
        let list = engine.db().list_markets()?;
        return json_bytes(&json!({ "count": list.len(), "markets": list }));
    }

    if req.method == "GET" && path == "/api/jurisdictions" {
        let list = engine.db().list_jurisdictions()?;
        return json_bytes(&json!({ "count": list.len(), "jurisdictions": list }));
    }

    if req.method == "GET" && path == "/api/injuries" {
        let cid = opt_q(&req.query, "character_id");
        let active_only = parse_opt_bool(&req.query, "active_only").unwrap_or(true);
        let list = engine.db().list_injuries(cid.as_deref(), active_only)?;
        return json_bytes(&json!({ "count": list.len(), "injuries": list }));
    }

    if req.method == "GET" && path.starts_with("/api/docs/") {
        let rel = url_decode(&path["/api/docs/".len()..]);
        if rel.contains("..") || rel.starts_with('/') || rel.starts_with('\\') {
            return Err(anyhow!("Invalid doc path"));
        }
        let docs_root = campaign_dir.join("docs");
        let target: PathBuf = docs_root.join(&rel);
        let text = std::fs::read_to_string(&target)
            .with_context(|| format!("Document not found: {}", rel))?;
        return Ok(("text/markdown; charset=utf-8".to_string(), text.into_bytes()));
    }

    if req.method == "POST" && path == "/api/context" {
        let body: Value = parse_json_body(&req.body)?;
        let max_chars = body
            .get("max_chars")
            .and_then(|v| v.as_u64())
            .map(|n| n as usize);
        let ctx_req: SceneContextRequest = serde_json::from_value(body)?;
        let packet = engine.get_scene_context(&ctx_req)?;
        let markdown = render_scene_context_markdown(&packet, max_chars);
        let approx_tokens_md = (markdown.len() + 3) / 4;
        let json_str = serde_json::to_string(&packet)?;
        let approx_tokens_json = (json_str.len() + 3) / 4;
        return json_bytes(&json!({
            "packet": packet,
            "markdown": markdown,
            "stats": {
                "markdown_chars": markdown.len(),
                "approx_markdown_tokens": approx_tokens_md,
                "json_chars": json_str.len(),
                "approx_json_tokens": approx_tokens_json
            }
        }));
    }

    // Universal tool invocation endpoint: POST /api/tool/<tool_name>
    if req.method == "POST" && path.starts_with("/api/tool/") {
        let tool_name = url_decode(&path["/api/tool/".len()..]);
        let body = if req.body.is_empty() {
            json!({})
        } else {
            parse_json_body(&req.body)?
        };
        let res = execute_tool_with_policy(&engine, &tool_name, body, allow_local_file_import)?;
        return json_bytes(&res);
    }

    Err(anyhow!("Not found: {} {}", req.method, req.path))
}

fn json_bytes(v: &Value) -> Result<(String, Vec<u8>)> {
    Ok((
        "application/json; charset=utf-8".to_string(),
        serde_json::to_vec(v)?,
    ))
}

fn parse_json_body(bytes: &[u8]) -> Result<Value> {
    if bytes.is_empty() {
        return Ok(json!({}));
    }
    serde_json::from_slice(bytes).context("Invalid JSON request body")
}

const MAX_HTTP_BODY_BYTES: usize = 16 * 1024 * 1024; // 16 MiB safety cap

fn read_http_request(stream: &mut TcpStream) -> Result<Option<HttpRequest>> {
    let mut reader = BufReader::new(stream.try_clone()?);
    let mut request_line = String::new();
    let n = reader.read_line(&mut request_line)?;
    if n == 0 {
        return Ok(None);
    }
    let parts: Vec<&str> = request_line.split_whitespace().collect();
    if parts.len() < 2 {
        return Ok(None);
    }
    let method = parts[0].to_uppercase();
    let target = parts[1];
    let (raw_path, raw_query) = match target.split_once('?') {
        Some((p, q)) => (p, Some(q)),
        None => (target, None),
    };

    let mut content_length = 0usize;
    let mut is_chunked = false;
    let mut expect_continue = false;
    let mut headers = BTreeMap::new();
    loop {
        let mut line = String::new();
        let read = reader.read_line(&mut line)?;
        if read == 0 || line == "\r\n" || line == "\n" {
            break;
        }
        if let Some((k, v)) = line.split_once(':') {
            let key = k.trim().to_lowercase();
            let val = v.trim().to_string();
            if key == "content-length" {
                content_length = val.parse().unwrap_or(0);
            } else if key == "transfer-encoding" && val.to_lowercase().contains("chunked") {
                is_chunked = true;
            } else if key == "expect" && val.to_lowercase().contains("100-continue") {
                expect_continue = true;
            }
            headers.insert(key, val);
        }
    }

    if expect_continue {
        stream.write_all(b"HTTP/1.1 100 Continue\r\n\r\n")?;
        stream.flush()?;
    }

    let body = if is_chunked {
        read_chunked_body(&mut reader)?
    } else if content_length > 0 {
        if content_length > MAX_HTTP_BODY_BYTES {
            return Err(anyhow!("HTTP request body exceeds {MAX_HTTP_BODY_BYTES} byte limit"));
        }
        let mut buf = vec![0u8; content_length];
        reader.read_exact(&mut buf)?;
        buf
    } else {
        Vec::new()
    };

    let mut query = BTreeMap::new();
    if let Some(q) = raw_query {
        for pair in q.split('&') {
            if pair.is_empty() {
                continue;
            }
            if let Some((k, v)) = pair.split_once('=') {
                query.insert(url_decode(k), url_decode(v));
            } else {
                query.insert(url_decode(pair), String::new());
            }
        }
    }

    Ok(Some(HttpRequest {
        method,
        path: raw_path.to_string(),
        query,
        headers,
        body,
    }))
}

/// Decodes RFC 9112 `Transfer-Encoding: chunked` HTTP/1.1 request bodies (commonly emitted by
/// reverse proxies such as `cloudflared` when forwarding HTTP/2 POST requests to HTTP/1.1 origins).
pub fn read_chunked_body<R: BufRead>(reader: &mut R) -> Result<Vec<u8>> {
    let mut out = Vec::new();
    loop {
        let mut size_line = String::new();
        let n = reader.read_line(&mut size_line)?;
        if n == 0 {
            break;
        }
        let trimmed = size_line.trim();
        if trimmed.is_empty() {
            continue;
        }
        // Strip optional chunk extensions (`<hex>;ext=val`)
        let hex_part = trimmed.split(';').next().unwrap_or("").trim();
        let chunk_size = usize::from_str_radix(hex_part, 16)
            .with_context(|| format!("Invalid HTTP chunk size: '{trimmed}'"))?;
        if chunk_size == 0 {
            // Drain optional trailer headers until empty line
            loop {
                let mut trailer = String::new();
                let tn = reader.read_line(&mut trailer)?;
                if tn == 0 || trailer == "\r\n" || trailer == "\n" {
                    break;
                }
            }
            break;
        }
        if out.len().saturating_add(chunk_size) > MAX_HTTP_BODY_BYTES {
            return Err(anyhow!("Chunked HTTP request body exceeds {MAX_HTTP_BODY_BYTES} byte limit"));
        }
        let start = out.len();
        out.resize(start + chunk_size, 0);
        reader.read_exact(&mut out[start..])?;
        // Consume trailing CRLF after chunk payload
        let mut crlf = String::new();
        let _ = reader.read_line(&mut crlf)?;
    }
    Ok(out)
}

fn send_response(
    stream: &mut TcpStream,
    status: u16,
    status_text: &str,
    content_type: &str,
    body: &[u8],
    extra_headers: &[(String, String)],
) -> Result<()> {
    let mut header = format!(
        "HTTP/1.1 {status} {status_text}\r\n\
         Content-Type: {content_type}\r\n\
         Content-Length: {}\r\n\
         Access-Control-Allow-Origin: *\r\n\
         Access-Control-Allow-Methods: GET, POST, DELETE, OPTIONS\r\n\
         Access-Control-Allow-Headers: Content-Type, Authorization, Accept, Mcp-Session-Id, MCP-Protocol-Version\r\n\
         Access-Control-Expose-Headers: Mcp-Session-Id\r\n",
        body.len()
    );
    for (k, v) in extra_headers {
        header.push_str(&format!("{k}: {v}\r\n"));
    }
    header.push_str("Connection: close\r\n\r\n");
    stream.write_all(header.as_bytes())?;
    stream.write_all(body)?;
    stream.flush()?;
    Ok(())
}

fn url_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'+' => {
                out.push(b' ');
                i += 1;
            }
            b'%' if i + 2 < bytes.len() => {
                if let Ok(hex) = std::str::from_utf8(&bytes[i + 1..i + 3]) {
                    if let Ok(val) = u8::from_str_radix(hex, 16) {
                        out.push(val);
                        i += 3;
                        continue;
                    }
                }
                out.push(bytes[i]);
                i += 1;
            }
            b => {
                out.push(b);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn opt_q(q: &BTreeMap<String, String>, key: &str) -> Option<String> {
    q.get(key).filter(|v| !v.trim().is_empty()).cloned()
}

fn parse_opt_i64(q: &BTreeMap<String, String>, key: &str) -> Option<i64> {
    q.get(key).and_then(|v| v.trim().parse().ok())
}

fn parse_opt_u32(q: &BTreeMap<String, String>, key: &str) -> Option<u32> {
    q.get(key).and_then(|v| v.trim().parse().ok())
}

fn parse_opt_u8(q: &BTreeMap<String, String>, key: &str) -> Option<u8> {
    q.get(key).and_then(|v| v.trim().parse().ok())
}

fn parse_opt_usize(q: &BTreeMap<String, String>, key: &str) -> Option<usize> {
    q.get(key).and_then(|v| v.trim().parse().ok())
}

fn parse_opt_bool(q: &BTreeMap<String, String>, key: &str) -> Option<bool> {
    q.get(key).and_then(|v| match v.trim().to_lowercase().as_str() {
        "true" | "1" | "yes" => Some(true),
        "false" | "0" | "no" => Some(false),
        _ => None,
    })
}

/// Generates an OpenAPI 3.1 specification covering every Chronicle tool endpoint
/// so Custom GPTs, LangChain, CrewAI, AutoGen, n8n, or SillyTavern can integrate effortlessly.
pub fn build_openapi_spec() -> Value {
    let mut paths = serde_json::Map::new();
    if let Value::Array(tools) = all_tool_definitions() {
        for t in tools {
            if let Some(name) = t.get("name").and_then(|v| v.as_str()) {
                let desc = t
                    .get("description")
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                let schema = t.get("inputSchema").cloned().unwrap_or_else(|| json!({}));
                paths.insert(
                    format!("/api/tool/{name}"),
                    json!({
                        "post": {
                            "operationId": name,
                            "summary": desc,
                            "requestBody": {
                                "required": true,
                                "content": {
                                    "application/json": { "schema": schema }
                                }
                            },
                            "responses": {
                                "200": { "description": "Successful Chronicle tool response" }
                            }
                        }
                    }),
                );
            }
        }
    }

    json!({
        "openapi": "3.1.0",
        "info": {
            "title": "Chronicle RPG GM Engine API",
            "version": env!("CARGO_PKG_VERSION"),
            "description": "Event-sourced RPG state engine with Knowledge Graph, FTS5 Lorebook, multi-dimensional Search By filters, Deterministic Dice, Time-Travel Rollback, and Auto-Compiled Markdown Docs."
        },
        "paths": Value::Object(paths)
    })
}
