use rand::Rng;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::time::{Duration, Instant};

/// In-memory OAuth 2.1 + PKCE + RFC 7591 Dynamic Client Registration store for Chronicle's
/// remote Streamable HTTP MCP server (`/mcp`).
///
/// Designed specifically for web-hosted / tunneled MCP deployments (e.g., Google Colab +
/// Cloudflare Tunnel / ngrok) connecting to ChatGPT Web/Desktop, Antigravity, or Claude.
#[derive(Debug)]
pub struct OAuthStore {
    /// Passphrase/secret required on the interactive `/oauth/authorize` consent page.
    pub admin_secret: String,
    clients: BTreeMap<String, RegisteredClient>,
    auth_codes: BTreeMap<String, AuthCodeRecord>,
    access_tokens: BTreeMap<String, TokenRecord>,
    refresh_tokens: BTreeMap<String, RefreshRecord>,
    /// Brute-force lockout state for `POST /oauth/authorize`.
    failed_auth_attempts: u32,
    auth_locked_until: Option<Instant>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RegisteredClient {
    pub client_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub client_secret: Option<String>,
    pub client_name: String,
    pub redirect_uris: Vec<String>,
    pub token_endpoint_auth_method: String,
}

#[derive(Debug, Clone)]
struct AuthCodeRecord {
    client_id: String,
    redirect_uri: String,
    code_challenge: Option<String>,
    code_challenge_method: String,
    scope: String,
    expires_at: Instant,
}

#[derive(Debug, Clone)]
struct TokenRecord {
    #[allow(dead_code)]
    client_id: String,
    #[allow(dead_code)]
    scope: String,
    expires_at: Instant,
}

#[derive(Debug, Clone)]
struct RefreshRecord {
    client_id: String,
    scope: String,
    expires_at: Instant,
}

pub struct OAuthHttpResponse {
    pub status: u16,
    pub status_text: &'static str,
    pub content_type: String,
    pub body: Vec<u8>,
    pub extra_headers: Vec<(String, String)>,
}

impl OAuthStore {
    pub fn new(admin_secret: String) -> Self {
        Self {
            admin_secret,
            clients: BTreeMap::new(),
            auth_codes: BTreeMap::new(),
            access_tokens: BTreeMap::new(),
            refresh_tokens: BTreeMap::new(),
            failed_auth_attempts: 0,
            auth_locked_until: None,
        }
    }

    pub fn generate_random_secret(prefix: &str) -> String {
        let mut rng = rand::thread_rng();
        let mut bytes = [0u8; 24];
        rng.fill(&mut bytes);
        format!("{prefix}{}", base64url_nopad(&bytes))
    }

    /// Validates whether a Bearer token matches an active, non-expired OAuth access token.
    pub fn validate_access_token(&mut self, token: &str) -> bool {
        let now = Instant::now();
        self.access_tokens.retain(|_, v| v.expires_at > now);
        self.access_tokens.contains_key(token)
    }

    /// RFC 9728 OAuth 2.0 Protected Resource Metadata
    pub fn protected_resource_metadata(origin: &str) -> Value {
        let base = origin.trim_end_matches('/');
        json!({
            "resource": format!("{base}/mcp"),
            "authorization_servers": [base],
            "bearer_methods_supported": ["header"],
            "scopes_supported": ["mcp:tools", "mcp:resources"]
        })
    }

    /// RFC 8414 OAuth 2.0 Authorization Server Metadata
    pub fn authorization_server_metadata(origin: &str) -> Value {
        let base = origin.trim_end_matches('/');
        json!({
            "issuer": base,
            "authorization_endpoint": format!("{base}/oauth/authorize"),
            "token_endpoint": format!("{base}/oauth/token"),
            "registration_endpoint": format!("{base}/oauth/register"),
            "response_types_supported": ["code"],
            "response_modes_supported": ["query"],
            "grant_types_supported": ["authorization_code", "refresh_token"],
            "code_challenge_methods_supported": ["S256", "plain"],
            "token_endpoint_auth_methods_supported": ["none", "client_secret_post", "client_secret_basic"],
            "scopes_supported": ["mcp:tools", "mcp:resources"]
        })
    }

    /// RFC 7591 Dynamic Client Registration (`POST /oauth/register`)
    pub fn handle_register(&mut self, body: &[u8]) -> OAuthHttpResponse {
        let payload: Value = match serde_json::from_slice(body) {
            Ok(v) => v,
            Err(e) => {
                return json_resp(
                    400,
                    "Bad Request",
                    json!({ "error": "invalid_client_metadata", "error_description": format!("Invalid JSON: {e}") }),
                );
            }
        };

        let redirect_uris: Vec<String> = payload
            .get("redirect_uris")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str().map(|s| s.to_string()))
                    .collect()
            })
            .unwrap_or_default();

        if redirect_uris.is_empty() {
            return json_resp(
                400,
                "Bad Request",
                json!({ "error": "invalid_redirect_uri", "error_description": "redirect_uris must contain at least one URI" }),
            );
        }

        let client_name = payload
            .get("client_name")
            .and_then(|v| v.as_str())
            .unwrap_or("MCP Client")
            .to_string();
        let auth_method = payload
            .get("token_endpoint_auth_method")
            .and_then(|v| v.as_str())
            .unwrap_or("none")
            .to_string();

        let client_id = Self::generate_random_secret("chr_cid_");
        let client_secret = if auth_method == "none" {
            None
        } else {
            Some(Self::generate_random_secret("chr_sec_"))
        };

        let reg = RegisteredClient {
            client_id: client_id.clone(),
            client_secret: client_secret.clone(),
            client_name: client_name.clone(),
            redirect_uris: redirect_uris.clone(),
            token_endpoint_auth_method: auth_method.clone(),
        };
        self.clients.insert(client_id.clone(), reg);

        let mut resp = json!({
            "client_id": client_id,
            "client_name": client_name,
            "redirect_uris": redirect_uris,
            "grant_types": ["authorization_code", "refresh_token"],
            "response_types": ["code"],
            "token_endpoint_auth_method": auth_method,
            "client_id_issued_at": chrono::Utc::now().timestamp()
        });
        if let Some(sec) = client_secret {
            resp["client_secret"] = Value::String(sec);
            resp["client_secret_expires_at"] = json!(0);
        }
        json_resp(201, "Created", resp)
    }

    /// Interactive Consent Screen (`GET /oauth/authorize`)
    pub fn handle_authorize_get(&self, query: &BTreeMap<String, String>) -> OAuthHttpResponse {
        let client_id = query.get("client_id").cloned().unwrap_or_default();
        let redirect_uri = query.get("redirect_uri").cloned().unwrap_or_default();
        let response_type = query
            .get("response_type")
            .cloned()
            .unwrap_or_else(|| "code".to_string());
        let state = query.get("state").cloned().unwrap_or_default();
        let scope = query
            .get("scope")
            .cloned()
            .unwrap_or_else(|| "mcp:tools mcp:resources".to_string());
        let code_challenge = query.get("code_challenge").cloned().unwrap_or_default();
        let code_challenge_method = query
            .get("code_challenge_method")
            .cloned()
            .unwrap_or_else(|| "S256".to_string());

        if response_type != "code" || client_id.is_empty() || redirect_uri.is_empty() {
            return html_resp(
                400,
                "Bad Request",
                "<h2>Invalid OAuth Authorization Request</h2><p>Required parameters: response_type=code, client_id, redirect_uri.</p>",
            );
        }

        // If client was dynamically registered, ensure redirect_uri matches one of its registered URIs
        let client_label = if let Some(c) = self.clients.get(&client_id) {
            if !c.redirect_uris.iter().any(|u| u == &redirect_uri) {
                return html_resp(
                    400,
                    "Bad Request",
                    "<h2>Redirect URI Mismatch</h2><p>The redirect_uri does not match the registered client.</p>",
                );
            }
            c.client_name.clone()
        } else {
            client_id.clone()
        };

        let html = render_consent_page(
            &client_label,
            &client_id,
            &redirect_uri,
            &state,
            &scope,
            &code_challenge,
            &code_challenge_method,
            None,
        );
        html_resp(200, "OK", &html)
    }

    /// Processes Consent Form Submission (`POST /oauth/authorize`)
    pub fn handle_authorize_post(&mut self, body: &[u8]) -> OAuthHttpResponse {
        let form = parse_form_urlencoded(&String::from_utf8_lossy(body));
        let client_id = form.get("client_id").cloned().unwrap_or_default();
        let redirect_uri = form.get("redirect_uri").cloned().unwrap_or_default();
        let state = form.get("state").cloned().unwrap_or_default();
        let scope = form
            .get("scope")
            .cloned()
            .unwrap_or_else(|| "mcp:tools mcp:resources".to_string());
        let code_challenge = form.get("code_challenge").cloned().unwrap_or_default();
        let code_challenge_method = form
            .get("code_challenge_method")
            .cloned()
            .unwrap_or_else(|| "S256".to_string());
        let passphrase = form.get("passphrase").cloned().unwrap_or_default();
        let decision = form
            .get("decision")
            .cloned()
            .unwrap_or_else(|| "approve".to_string());

        if client_id.is_empty() || redirect_uri.is_empty() {
            return html_resp(400, "Bad Request", "<h2>Missing client_id or redirect_uri</h2>");
        }

        if let Some(c) = self.clients.get(&client_id) {
            if !c.redirect_uris.iter().any(|u| u == &redirect_uri) {
                return html_resp(400, "Bad Request", "<h2>Redirect URI mismatch</h2>");
            }
        }

        if decision == "deny" {
            let loc = append_query_params(
                &redirect_uri,
                &[
                    ("error", "access_denied"),
                    ("state", state.as_str()),
                ],
            );
            return redirect_resp(&loc);
        }

        if let Some(until) = self.auth_locked_until {
            let now = Instant::now();
            if until > now {
                let secs = (until - now).as_secs().max(1);
                return OAuthHttpResponse {
                    status: 429,
                    status_text: "Too Many Requests",
                    content_type: "text/html; charset=utf-8".to_string(),
                    body: format!(
                        "<h2>Too Many Failed Attempts</h2><p>Passphrase entry temporarily locked for {secs}s after repeated failed attempts.</p>"
                    )
                    .into_bytes(),
                    extra_headers: vec![("Retry-After".to_string(), secs.to_string())],
                };
            } else {
                self.auth_locked_until = None;
                self.failed_auth_attempts = 0;
            }
        }

        if !constant_time_eq(passphrase.trim().as_bytes(), self.admin_secret.as_bytes()) {
            self.failed_auth_attempts += 1;
            let locked_msg = if self.failed_auth_attempts >= 5 {
                self.auth_locked_until = Some(Instant::now() + Duration::from_secs(60));
                "Invalid GM OAuth passphrase. Too many failed attempts — locked for 60 seconds."
            } else {
                "Invalid GM OAuth passphrase. Check your Chronicle server startup output."
            };
            let client_label = self
                .clients
                .get(&client_id)
                .map(|c| c.client_name.clone())
                .unwrap_or_else(|| client_id.clone());
            let html = render_consent_page(
                &client_label,
                &client_id,
                &redirect_uri,
                &state,
                &scope,
                &code_challenge,
                &code_challenge_method,
                Some(locked_msg),
            );
            return if self.failed_auth_attempts >= 5 {
                OAuthHttpResponse {
                    status: 429,
                    status_text: "Too Many Requests",
                    content_type: "text/html; charset=utf-8".to_string(),
                    body: html.into_bytes(),
                    extra_headers: vec![("Retry-After".to_string(), "60".to_string())],
                }
            } else {
                html_resp(403, "Forbidden", &html)
            };
        }

        self.failed_auth_attempts = 0;
        self.auth_locked_until = None;

        // Prune expired auth codes and mint a single-use code valid for 5 minutes
        let now = Instant::now();
        self.auth_codes.retain(|_, rec| rec.expires_at > now);

        let code = Self::generate_random_secret("chr_code_");
        self.auth_codes.insert(
            code.clone(),
            AuthCodeRecord {
                client_id,
                redirect_uri: redirect_uri.clone(),
                code_challenge: if code_challenge.is_empty() {
                    None
                } else {
                    Some(code_challenge)
                },
                code_challenge_method,
                scope,
                expires_at: now + Duration::from_secs(300),
            },
        );

        let loc = append_query_params(
            &redirect_uri,
            &[("code", code.as_str()), ("state", state.as_str())],
        );
        redirect_resp(&loc)
    }

    /// Token Exchange & Refresh (`POST /oauth/token`)
    pub fn handle_token(
        &mut self,
        headers: &BTreeMap<String, String>,
        body: &[u8],
    ) -> OAuthHttpResponse {
        let ct = headers
            .get("content-type")
            .map(|s| s.to_lowercase())
            .unwrap_or_default();

        let params: BTreeMap<String, String> = if ct.contains("application/json") {
            match serde_json::from_slice::<Value>(body) {
                Ok(Value::Object(map)) => map
                    .into_iter()
                    .filter_map(|(k, v)| v.as_str().map(|s| (k, s.to_string())))
                    .collect(),
                _ => BTreeMap::new(),
            }
        } else {
            parse_form_urlencoded(&String::from_utf8_lossy(body))
        };

        let grant_type = params.get("grant_type").map(|s| s.as_str()).unwrap_or("");
        let now = Instant::now();
        self.auth_codes.retain(|_, v| v.expires_at > now);
        self.access_tokens.retain(|_, v| v.expires_at > now);
        self.refresh_tokens.retain(|_, v| v.expires_at > now);

        match grant_type {
            "authorization_code" => {
                let code = params.get("code").cloned().unwrap_or_default();
                let redirect_uri = params.get("redirect_uri").cloned().unwrap_or_default();
                let code_verifier = params.get("code_verifier").cloned().unwrap_or_default();

                let Some(rec) = self.auth_codes.remove(&code) else {
                    return json_resp(
                        400,
                        "Bad Request",
                        json!({ "error": "invalid_grant", "error_description": "Authorization code is invalid or expired" }),
                    );
                };

                if !redirect_uri.is_empty() && redirect_uri != rec.redirect_uri {
                    return json_resp(
                        400,
                        "Bad Request",
                        json!({ "error": "invalid_grant", "error_description": "redirect_uri mismatch" }),
                    );
                }

                // Verify PKCE code_verifier if code_challenge was provided during /oauth/authorize
                if let Some(ref challenge) = rec.code_challenge {
                    let ok = match rec.code_challenge_method.as_str() {
                        "S256" => {
                            let digest = Sha256::digest(code_verifier.as_bytes());
                            let computed = base64url_nopad(&digest);
                            constant_time_eq(computed.as_bytes(), challenge.as_bytes())
                        }
                        "plain" => constant_time_eq(code_verifier.as_bytes(), challenge.as_bytes()),
                        _ => false,
                    };
                    if !ok {
                        return json_resp(
                            400,
                            "Bad Request",
                            json!({ "error": "invalid_grant", "error_description": "PKCE code_verifier verification failed" }),
                        );
                    }
                }

                self.mint_tokens(&rec.client_id, &rec.scope)
            }
            "refresh_token" => {
                let rt = params.get("refresh_token").cloned().unwrap_or_default();
                let Some(rec) = self.refresh_tokens.remove(&rt) else {
                    return json_resp(
                        400,
                        "Bad Request",
                        json!({ "error": "invalid_grant", "error_description": "Refresh token is invalid or expired" }),
                    );
                };
                self.mint_tokens(&rec.client_id, &rec.scope)
            }
            other => json_resp(
                400,
                "Bad Request",
                json!({ "error": "unsupported_grant_type", "error_description": format!("Unsupported grant_type '{other}'") }),
            ),
        }
    }

    fn mint_tokens(&mut self, client_id: &str, scope: &str) -> OAuthHttpResponse {
        let now = Instant::now();
        let access_token = Self::generate_random_secret("chr_at_");
        let refresh_token = Self::generate_random_secret("chr_rt_");
        let expires_in_secs = 86_400u64; // 24 hours

        self.access_tokens.insert(
            access_token.clone(),
            TokenRecord {
                client_id: client_id.to_string(),
                scope: scope.to_string(),
                expires_at: now + Duration::from_secs(expires_in_secs),
            },
        );
        self.refresh_tokens.insert(
            refresh_token.clone(),
            RefreshRecord {
                client_id: client_id.to_string(),
                scope: scope.to_string(),
                expires_at: now + Duration::from_secs(86_400 * 30),
            },
        );

        json_resp(
            200,
            "OK",
            json!({
                "access_token": access_token,
                "token_type": "Bearer",
                "expires_in": expires_in_secs,
                "refresh_token": refresh_token,
                "scope": scope
            }),
        )
    }
}

fn json_resp(status: u16, status_text: &'static str, body: Value) -> OAuthHttpResponse {
    OAuthHttpResponse {
        status,
        status_text,
        content_type: "application/json; charset=utf-8".to_string(),
        body: serde_json::to_vec(&body).unwrap_or_default(),
        extra_headers: vec![("Cache-Control".to_string(), "no-store".to_string())],
    }
}

fn html_resp(status: u16, status_text: &'static str, html: &str) -> OAuthHttpResponse {
    OAuthHttpResponse {
        status,
        status_text,
        content_type: "text/html; charset=utf-8".to_string(),
        body: html.as_bytes().to_vec(),
        extra_headers: vec![("Cache-Control".to_string(), "no-store".to_string())],
    }
}

fn redirect_resp(location: &str) -> OAuthHttpResponse {
    OAuthHttpResponse {
        status: 302,
        status_text: "Found",
        content_type: "text/plain; charset=utf-8".to_string(),
        body: format!("Redirecting to {location}").into_bytes(),
        extra_headers: vec![
            ("Location".to_string(), location.to_string()),
            ("Cache-Control".to_string(), "no-store".to_string()),
        ],
    }
}

#[allow(clippy::too_many_arguments)]
fn render_consent_page(
    client_name: &str,
    client_id: &str,
    redirect_uri: &str,
    state: &str,
    scope: &str,
    code_challenge: &str,
    code_challenge_method: &str,
    error_msg: Option<&str>,
) -> String {
    let err_html = match error_msg {
        Some(m) => format!(
            "<div style='background:#3f1d24;border:1px solid #f87171;color:#fecaca;padding:10px;border-radius:8px;margin-bottom:14px;font-size:13px;'>{}</div>",
            escape_html(m)
        ),
        None => String::new(),
    };
    format!(
        r#"<!DOCTYPE html>
<html lang="en">
<head>
<meta charset="UTF-8" />
<meta name="viewport" content="width=device-width, initial-scale=1.0" />
<title>Authorize Chronicle MCP Client</title>
<style>
  body {{ margin:0; font-family:system-ui,-apple-system,sans-serif; background:#0c0f16; color:#e6ebf5; display:grid; place-items:center; min-height:100vh; padding:16px; }}
  .card {{ background:#171d2b; border:1px solid #28324a; border-radius:12px; padding:24px; max-width:460px; width:100%; box-shadow:0 16px 40px rgba(0,0,0,0.45); }}
  h1 {{ margin:0 0 6px 0; font-size:20px; color:#e89b3c; }}
  p {{ margin:0 0 14px 0; color:#8f9bb3; font-size:13.5px; line-height:1.5; }}
  .meta {{ background:#0d121c; border:1px solid #28324a; border-radius:8px; padding:10px 12px; margin-bottom:14px; font-size:12.5px; }}
  .meta div {{ margin:4px 0; word-break:break-all; }}
  label {{ display:block; font-size:12.5px; color:#cbd5e1; margin-bottom:6px; font-weight:600; }}
  input[type=password] {{ width:100%; box-sizing:border-box; padding:10px 12px; border-radius:8px; border:1px solid #334155; background:#0d121c; color:#fff; font-size:14px; margin-bottom:14px; }}
  .row {{ display:flex; gap:10px; }}
  button {{ flex:1; padding:10px 14px; border-radius:8px; border:1px solid #334155; background:#1e2638; color:#e6ebf5; font-weight:600; cursor:pointer; font-size:14px; }}
  button.primary {{ background:#e89b3c; color:#111; border-color:#e89b3c; }}
</style>
</head>
<body>
  <div class="card">
    <h1>Chronicle RPG MCP Authorization</h1>
    <p>An AI client is requesting OAuth 2.1 access to your hosted Chronicle campaign server.</p>
    {err_html}
    <div class="meta">
      <div><strong>Client:</strong> {cname}</div>
      <div><strong>Scope:</strong> {scope}</div>
      <div><strong>Redirect:</strong> {ruri}</div>
    </div>
    <form method="POST" action="/oauth/authorize">
      <input type="hidden" name="client_id" value="{cid}" />
      <input type="hidden" name="redirect_uri" value="{ruri}" />
      <input type="hidden" name="state" value="{state}" />
      <input type="hidden" name="scope" value="{scope}" />
      <input type="hidden" name="code_challenge" value="{cc}" />
      <input type="hidden" name="code_challenge_method" value="{ccm}" />
      <label for="passphrase">GM OAuth Passphrase (from server startup log / CHRONICLE_OAUTH_SECRET)</label>
      <input id="passphrase" name="passphrase" type="password" required autofocus placeholder="Enter GM passphrase..." />
      <div class="row">
        <button type="submit" name="decision" value="deny">Deny</button>
        <button type="submit" name="decision" value="approve" class="primary">Authorize &amp; Connect</button>
      </div>
    </form>
  </div>
</body>
</html>"#,
        err_html = err_html,
        cname = escape_html(client_name),
        scope = escape_html(scope),
        ruri = escape_html(redirect_uri),
        cid = escape_html(client_id),
        state = escape_html(state),
        cc = escape_html(code_challenge),
        ccm = escape_html(code_challenge_method),
    )
}

fn escape_html(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}

fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for (x, y) in a.iter().zip(b.iter()) {
        diff |= x ^ y;
    }
    diff == 0
}

fn append_query_params(base: &str, params: &[(&str, &str)]) -> String {
    let mut out = base.to_string();
    let mut has_q = out.contains('?');
    for (k, v) in params {
        if v.is_empty() {
            continue;
        }
        out.push(if has_q { '&' } else { '?' });
        has_q = true;
        out.push_str(&url_encode(k));
        out.push('=');
        out.push_str(&url_encode(v));
    }
    out
}

fn url_encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~') {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

pub fn parse_form_urlencoded(raw: &str) -> BTreeMap<String, String> {
    let mut map = BTreeMap::new();
    for pair in raw.split('&') {
        if pair.is_empty() {
            continue;
        }
        if let Some((k, v)) = pair.split_once('=') {
            map.insert(url_decode(k), url_decode(v));
        } else {
            map.insert(url_decode(pair), String::new());
        }
    }
    map
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

/// Zero-dependency RFC 4648 base64url encoder without `=` padding (used for PKCE S256 & tokens).
pub fn base64url_nopad(input: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    let mut out = String::with_capacity((input.len() * 4).div_ceil(3));
    let mut i = 0;
    while i + 3 <= input.len() {
        let n = ((input[i] as u32) << 16) | ((input[i + 1] as u32) << 8) | (input[i + 2] as u32);
        out.push(TABLE[((n >> 18) & 63) as usize] as char);
        out.push(TABLE[((n >> 12) & 63) as usize] as char);
        out.push(TABLE[((n >> 6) & 63) as usize] as char);
        out.push(TABLE[(n & 63) as usize] as char);
        i += 3;
    }
    let rem = input.len() - i;
    if rem == 1 {
        let n = (input[i] as u32) << 16;
        out.push(TABLE[((n >> 18) & 63) as usize] as char);
        out.push(TABLE[((n >> 12) & 63) as usize] as char);
    } else if rem == 2 {
        let n = ((input[i] as u32) << 16) | ((input[i + 1] as u32) << 8);
        out.push(TABLE[((n >> 18) & 63) as usize] as char);
        out.push(TABLE[((n >> 12) & 63) as usize] as char);
        out.push(TABLE[((n >> 6) & 63) as usize] as char);
    }
    out
}
