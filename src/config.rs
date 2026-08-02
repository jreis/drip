//! Local config: Raindrop OAuth credentials + tokens.

use anyhow::{Context, Result, bail};
use chrono::{DateTime, Duration, Utc};
use directories::ProjectDirs;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;

/// Default localhost callback — set this exact URL in your Raindrop app settings.
pub const DEFAULT_REDIRECT_URI: &str = "http://127.0.0.1:8787/callback";
pub const DEFAULT_OAUTH_PORT: u16 = 8787;

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Config {
    /// OAuth application client id.
    #[serde(default)]
    pub client_id: Option<String>,
    /// OAuth application client secret.
    #[serde(default)]
    pub client_secret: Option<String>,
    /// Must match the redirect URL registered on the Raindrop app.
    #[serde(default)]
    pub redirect_uri: Option<String>,

    /// OAuth access token (expires ~2 weeks).
    #[serde(default)]
    pub access_token: Option<String>,
    /// OAuth refresh token.
    #[serde(default)]
    pub refresh_token: Option<String>,
    /// When access_token is considered expired (UTC).
    #[serde(default)]
    pub token_expires_at: Option<DateTime<Utc>>,

    /// Optional long-lived Test token from app settings (no refresh).
    /// Also overridable via env `RAINDROP_TOKEN`.
    #[serde(default)]
    pub raindrop_token: Option<String>,

    /// Pull from API when the TUI starts (opt-in).
    #[serde(default)]
    pub sync_on_start: bool,
}

impl Config {
    pub fn redirect_uri(&self) -> String {
        self.redirect_uri
            .as_deref()
            .filter(|s| !s.is_empty())
            .unwrap_or(DEFAULT_REDIRECT_URI)
            .to_string()
    }

    pub fn has_oauth_app(&self) -> bool {
        self.client_id.as_ref().is_some_and(|s| !s.is_empty())
            && self.client_secret.as_ref().is_some_and(|s| !s.is_empty())
    }

    /// Prefer env test token, then stored test token, then OAuth access token.
    pub fn static_token(&self) -> Option<String> {
        std::env::var("RAINDROP_TOKEN")
            .ok()
            .filter(|s| !s.trim().is_empty())
            .or_else(|| {
                self.raindrop_token
                    .as_ref()
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty())
            })
    }

    pub fn access_token_valid(&self) -> Option<&str> {
        let tok = self.access_token.as_deref()?.trim();
        if tok.is_empty() {
            return None;
        }
        if let Some(exp) = self.token_expires_at {
            // Refresh 5 minutes early.
            if Utc::now() + Duration::minutes(5) >= exp {
                return None;
            }
        }
        Some(tok)
    }

    pub fn apply_token_response(
        &mut self,
        access_token: String,
        refresh_token: Option<String>,
        expires_in: Option<i64>,
    ) {
        self.access_token = Some(access_token);
        if let Some(r) = refresh_token.filter(|s| !s.is_empty()) {
            self.refresh_token = Some(r);
        }
        if let Some(secs) = expires_in {
            self.token_expires_at = Some(Utc::now() + Duration::seconds(secs.max(60)));
        } else {
            // Raindrop default ~14 days; be conservative if missing.
            self.token_expires_at = Some(Utc::now() + Duration::days(13));
        }
    }
}

pub fn config_dir() -> Result<PathBuf> {
    let dirs =
        ProjectDirs::from("io", "drip", "drip").context("could not resolve config directory")?;
    let dir = dirs.config_dir().to_path_buf();
    fs::create_dir_all(&dir).with_context(|| format!("create {}", dir.display()))?;
    Ok(dir)
}

pub fn config_path() -> Result<PathBuf> {
    Ok(config_dir()?.join("config.json"))
}

pub fn load() -> Result<Config> {
    let path = config_path()?;
    if !path.exists() {
        return Ok(Config::default());
    }
    let raw = fs::read_to_string(&path).with_context(|| format!("read {}", path.display()))?;
    let cfg: Config =
        serde_json::from_str(&raw).with_context(|| format!("parse {}", path.display()))?;
    Ok(cfg)
}

pub fn save(cfg: &Config) -> Result<()> {
    let path = config_path()?;
    let raw = serde_json::to_string_pretty(cfg).context("serialize config")?;
    let tmp = path.with_extension("json.tmp");
    fs::write(&tmp, raw).with_context(|| format!("write {}", tmp.display()))?;
    fs::rename(&tmp, &path).with_context(|| format!("rename {}", path.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(&path, fs::Permissions::from_mode(0o600));
    }
    Ok(())
}

pub fn save_oauth_app(
    client_id: String,
    client_secret: String,
    redirect_uri: Option<String>,
) -> Result<Config> {
    let mut cfg = load()?;
    cfg.client_id = Some(client_id.trim().to_string());
    cfg.client_secret = Some(client_secret.trim().to_string());
    if let Some(r) = redirect_uri {
        let r = r.trim().to_string();
        if !r.is_empty() {
            cfg.redirect_uri = Some(r);
        }
    }
    if cfg.redirect_uri.is_none() {
        cfg.redirect_uri = Some(DEFAULT_REDIRECT_URI.into());
    }
    save(&cfg)?;
    Ok(cfg)
}

pub fn set_test_token(token: String) -> Result<Config> {
    let token = token.trim().to_string();
    if token.is_empty() {
        bail!("empty token");
    }
    let mut cfg = load()?;
    cfg.raindrop_token = Some(token);
    save(&cfg)?;
    Ok(cfg)
}

pub fn logout() -> Result<()> {
    let mut cfg = load()?;
    cfg.access_token = None;
    cfg.refresh_token = None;
    cfg.token_expires_at = None;
    cfg.raindrop_token = None;
    save(&cfg)?;
    Ok(())
}

/// Resolve a usable bearer token, refreshing OAuth if needed.
pub fn require_access_token() -> Result<String> {
    let mut cfg = load()?;

    if let Some(t) = cfg.static_token() {
        return Ok(t);
    }

    if let Some(t) = cfg.access_token_valid() {
        return Ok(t.to_string());
    }

    // Try refresh.
    if cfg.has_oauth_app()
        && let Some(refresh) = cfg.refresh_token.clone()
    {
        match crate::oauth::refresh_access_token(&cfg, &refresh) {
            Ok(tokens) => {
                cfg.apply_token_response(
                    tokens.access_token,
                    tokens.refresh_token,
                    tokens.expires_in,
                );
                save(&cfg)?;
                return Ok(cfg.access_token.clone().unwrap());
            }
            Err(e) => {
                eprintln!("token refresh failed: {e}");
            }
        }
    }

    bail!(
        "not authenticated with Raindrop.\n\
         \n\
         OAuth (recommended — you have client id/secret):\n\
           drip auth --client-id YOUR_ID --client-secret YOUR_SECRET\n\
         \n\
         Or paste a Test token from the app settings:\n\
           drip auth --token YOUR_TEST_TOKEN\n\
         \n\
         Config: {}",
        config_path()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|_| "(unknown)".into())
    )
}

pub fn auth_status_lines() -> Result<Vec<String>> {
    let cfg = load()?;
    let mut lines = Vec::new();
    lines.push(format!("config: {}", config_path()?.display()));

    match (&cfg.client_id, &cfg.client_secret) {
        (Some(id), Some(_)) => {
            let short = if id.len() > 8 {
                format!("{}…", &id[..8])
            } else {
                id.clone()
            };
            lines.push(format!("oauth app: client_id={short}"));
            lines.push(format!("redirect:  {}", cfg.redirect_uri()));
        }
        _ => lines.push("oauth app: not configured".into()),
    }

    if cfg.static_token().is_some() {
        lines.push("token:     test/env token present".into());
    } else if let Some(exp) = cfg.token_expires_at {
        if cfg.access_token_valid().is_some() {
            lines.push(format!(
                "token:     oauth access valid until {}",
                exp.format("%Y-%m-%d %H:%M UTC")
            ));
        } else {
            lines.push(format!(
                "token:     oauth access expired at {} (will refresh)",
                exp.format("%Y-%m-%d %H:%M UTC")
            ));
        }
        if cfg.refresh_token.is_some() {
            lines.push("refresh:   present".into());
        }
    } else if cfg.access_token.is_some() {
        lines.push("token:     oauth access present (no expiry stored)".into());
    } else {
        lines.push("token:     none — run drip auth".into());
    }

    Ok(lines)
}
