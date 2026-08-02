//! Raindrop OAuth2 authorization-code flow for CLI.

use crate::config::{self, Config, DEFAULT_OAUTH_PORT};
use anyhow::{Context, Result, bail};
use serde::Deserialize;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::time::Duration;
use ureq::Agent;

// Browser authorize still lives on the main site (docs).
const AUTHORIZE_URL: &str = "https://raindrop.io/oauth/authorize";
// Token exchange: raindrop.io returns 307 → api.raindrop.io; ureq fails following
// POST redirects ("redirect failed"). Hit the API host directly.
const TOKEN_URL: &str = "https://api.raindrop.io/v1/oauth/access_token";

#[derive(Debug, Clone)]
pub struct TokenResponse {
    pub access_token: String,
    pub refresh_token: Option<String>,
    pub expires_in: Option<i64>,
}

#[derive(Debug, Deserialize)]
struct TokenJson {
    access_token: Option<String>,
    refresh_token: Option<String>,
    expires_in: Option<i64>,
    error: Option<String>,
    #[serde(default)]
    error_description: Option<String>,
}

fn agent() -> Agent {
    Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(30)))
        .http_status_as_error(false)
        .user_agent("drip/0.1")
        .build()
        .into()
}

fn url_encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len() * 2);
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char);
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

pub fn authorize_url(client_id: &str, redirect_uri: &str) -> String {
    format!(
        "{AUTHORIZE_URL}?response_type=code&client_id={}&redirect_uri={}",
        url_encode(client_id),
        url_encode(redirect_uri)
    )
}

fn parse_token_body(body: &str) -> Result<TokenResponse> {
    let parsed: TokenJson = serde_json::from_str(body).context("parse token response JSON")?;
    if let Some(err) = parsed.error {
        let desc = parsed.error_description.unwrap_or_default();
        bail!("oauth error: {err} {desc}");
    }
    let access = parsed
        .access_token
        .filter(|s| !s.is_empty())
        .context("token response missing access_token")?;
    Ok(TokenResponse {
        access_token: access,
        refresh_token: parsed.refresh_token,
        expires_in: parsed.expires_in,
    })
}

pub fn exchange_code(cfg: &Config, code: &str) -> Result<TokenResponse> {
    let client_id = cfg.client_id.as_deref().context("missing client_id")?;
    let client_secret = cfg
        .client_secret
        .as_deref()
        .context("missing client_secret")?;
    let redirect_uri = cfg.redirect_uri();

    let payload = serde_json::json!({
        "grant_type": "authorization_code",
        "code": code,
        "client_id": client_id,
        "client_secret": client_secret,
        "redirect_uri": redirect_uri,
    });

    let mut resp = agent()
        .post(TOKEN_URL)
        .header("Content-Type", "application/json")
        .send(payload.to_string())
        .with_context(|| format!("POST {TOKEN_URL} (authorization_code)"))?;

    let status = resp.status().as_u16();
    let body = resp
        .body_mut()
        .read_to_string()
        .context("read token body")?;
    if !(200..300).contains(&status) {
        bail!("token exchange HTTP {status}: {body}");
    }
    parse_token_body(&body)
}

pub fn refresh_access_token(cfg: &Config, refresh_token: &str) -> Result<TokenResponse> {
    let client_id = cfg.client_id.as_deref().context("missing client_id")?;
    let client_secret = cfg
        .client_secret
        .as_deref()
        .context("missing client_secret")?;

    let payload = serde_json::json!({
        "grant_type": "refresh_token",
        "client_id": client_id,
        "client_secret": client_secret,
        "refresh_token": refresh_token,
    });

    let mut resp = agent()
        .post(TOKEN_URL)
        .header("Content-Type", "application/json")
        .send(payload.to_string())
        .with_context(|| format!("POST {TOKEN_URL} (refresh_token)"))?;

    let status = resp.status().as_u16();
    let body = resp
        .body_mut()
        .read_to_string()
        .context("read refresh body")?;
    if !(200..300).contains(&status) {
        bail!("token refresh HTTP {status}: {body}");
    }
    parse_token_body(&body)
}

/// Listen on 127.0.0.1:port for a single OAuth redirect with `?code=`.
pub fn wait_for_redirect_code(port: u16, timeout: Duration) -> Result<String> {
    let listener = TcpListener::bind(("127.0.0.1", port))
        .with_context(|| format!("bind 127.0.0.1:{port} — is another drip auth running?"))?;
    listener
        .set_nonblocking(false)
        .context("set listener blocking")?;

    // Accept with a manual deadline via OS timeout on the socket read after accept.
    // For simplicity, use a thread + channel with overall timeout.
    let (tx, rx) = std::sync::mpsc::channel::<Result<String>>();
    std::thread::spawn(move || {
        let result = (|| -> Result<String> {
            let (mut stream, _) = listener.accept().context("accept oauth callback")?;
            let mut buf = [0u8; 4096];
            let n = stream
                .read(&mut buf)
                .context("read oauth callback request")?;
            let req = String::from_utf8_lossy(&buf[..n]);
            let first_line = req.lines().next().unwrap_or("");
            // GET /callback?code=... HTTP/1.1
            let path = first_line.split_whitespace().nth(1).unwrap_or("");
            let code = extract_query_param(path, "code")
                .context("no ?code= in redirect — check redirect URI matches app settings")?;
            if let Some(err) = extract_query_param(path, "error") {
                let desc = extract_query_param(path, "error_description").unwrap_or_default();
                bail!("authorization denied: {err} {desc}");
            }

            let html = r#"<!doctype html>
<html><head><meta charset="utf-8"><title>drip</title></head>
<body style="font-family:system-ui;padding:2rem">
  <h1>Authenticated</h1>
  <p>You can close this tab and return to the terminal.</p>
</body></html>"#;
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                html.len(),
                html
            );
            let _ = stream.write_all(response.as_bytes());
            let _ = stream.flush();
            Ok(code)
        })();
        let _ = tx.send(result);
    });

    match rx.recv_timeout(timeout) {
        Ok(r) => r,
        Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
            bail!(
                "timed out waiting for browser redirect on port {port}.\n\
                 Make sure the Raindrop app redirect URL is exactly:\n\
                   http://127.0.0.1:{port}/callback"
            )
        }
        Err(e) => bail!("oauth callback channel error: {e}"),
    }
}

fn extract_query_param(path: &str, key: &str) -> Option<String> {
    let q = path.split_once('?')?.1;
    for pair in q.split('&') {
        let mut it = pair.splitn(2, '=');
        let k = it.next()?;
        let v = it.next().unwrap_or("");
        if k == key {
            return Some(url_decode(v));
        }
    }
    None
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
                let hex = &s[i + 1..i + 3];
                if let Ok(v) = u8::from_str_radix(hex, 16) {
                    out.push(v);
                    i += 3;
                } else {
                    out.push(b'%');
                    i += 1;
                }
            }
            b => {
                out.push(b);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Full interactive OAuth login: open browser, catch redirect, exchange code, save tokens.
pub fn login_interactive(cfg: &mut Config) -> Result<()> {
    if !cfg.has_oauth_app() {
        bail!(
            "client id/secret not set.\n\
             Run:\n\
               drip auth --client-id YOUR_ID --client-secret YOUR_SECRET"
        );
    }

    let redirect = cfg.redirect_uri();
    let port = parse_port_from_redirect(&redirect).unwrap_or(DEFAULT_OAUTH_PORT);

    // Ensure redirect uses our listener host/port convention when using default.
    if !redirect.contains(&format!("127.0.0.1:{port}"))
        && !redirect.contains(&format!("localhost:{port}"))
    {
        eprintln!(
            "warning: redirect_uri is {redirect}\n\
             drip will listen on 127.0.0.1:{port}.\n\
             If the Raindrop app uses a different URL, pass --redirect-uri and --port."
        );
    }

    let client_id = cfg.client_id.clone().unwrap();
    let url = authorize_url(&client_id, &redirect);

    println!("Opening browser for Raindrop authorization…");
    println!("If it does not open, visit:\n  {url}\n");
    println!(
        "Listening on http://127.0.0.1:{port}/callback  (must match your Raindrop app redirect URL)"
    );

    if let Err(e) = open::that(&url) {
        eprintln!("could not open browser automatically: {e}");
    }

    let code = wait_for_redirect_code(port, Duration::from_secs(5 * 60))?;
    println!("got authorization code — exchanging for tokens…");

    let tokens = exchange_code(cfg, &code)?;
    cfg.apply_token_response(tokens.access_token, tokens.refresh_token, tokens.expires_in);
    config::save(cfg)?;

    // Verify.
    let mut client = crate::raindrop::Client::new(cfg.access_token.clone().unwrap());
    match client.whoami() {
        Ok(who) => println!("authenticated as {who}"),
        Err(e) => println!("tokens saved, but whoami failed: {e}"),
    }
    println!("saved → {}", config::config_path()?.display());
    Ok(())
}

fn parse_port_from_redirect(redirect: &str) -> Option<u16> {
    // http://127.0.0.1:8787/callback
    let after_scheme = redirect.split("://").nth(1)?;
    let hostport = after_scheme.split('/').next()?;
    let port = hostport.split(':').nth(1)?;
    port.parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extract_code_from_callback_path() {
        let path = "/callback?code=abc%2D123&state=x";
        assert_eq!(
            extract_query_param(path, "code").as_deref(),
            Some("abc-123")
        );
    }

    #[test]
    fn authorize_url_encodes_redirect() {
        let url = authorize_url("cid", "http://127.0.0.1:8787/callback");
        assert!(url.contains("client_id=cid"));
        assert!(url.contains("redirect_uri=http%3A%2F%2F127.0.0.1%3A8787%2Fcallback"));
        assert!(url.contains("response_type=code"));
    }

    #[test]
    fn parse_port() {
        assert_eq!(
            parse_port_from_redirect("http://127.0.0.1:8787/callback"),
            Some(8787)
        );
    }
}
