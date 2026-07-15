use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Serialize, Deserialize)]
pub struct Config {
    pub app_key: Option<String>,
    pub app_secret: Option<String>,
    #[serde(default = "default_redirect_uri")]
    pub redirect_uri: String,
    pub access_token: Option<String>,
    pub refresh_token: Option<String>,
    pub token_expiry: Option<DateTime<Utc>>,
    /// Schwab's encrypted account hash for the account picked in AccountSelect,
    /// remembered so repeat launches don't force re-selection.
    pub selected_account_hash: Option<String>,
}

fn default_redirect_uri() -> String {
    "https://127.0.0.1".to_string()
}

impl Default for Config {
    fn default() -> Self {
        Self {
            app_key: None,
            app_secret: None,
            redirect_uri: default_redirect_uri(),
            access_token: None,
            refresh_token: None,
            token_expiry: None,
            selected_account_hash: None,
        }
    }
}

impl Config {
    fn path() -> Result<PathBuf> {
        let dir = dirs::config_dir()
            .context("Cannot locate config directory")?
            .join("schwab-cli");
        std::fs::create_dir_all(&dir)?;
        Ok(dir.join("config.json"))
    }

    pub fn load() -> Result<Self> {
        let path = Self::path()?;
        if !path.exists() {
            return Ok(Config::default());
        }
        let data = std::fs::read_to_string(&path)?;
        Ok(serde_json::from_str(&data)?)
    }

    pub fn save(&self) -> Result<()> {
        let path = Self::path()?;
        std::fs::write(path, serde_json::to_string_pretty(self)?)?;
        Ok(())
    }

    pub fn is_token_expired(&self) -> bool {
        match self.token_expiry {
            None => true,
            Some(expiry) => Utc::now() >= expiry - chrono::Duration::seconds(60),
        }
    }
}

pub fn print_status() -> Result<()> {
    let cfg = Config::load()?;
    println!("App key:      {}", cfg.app_key.as_deref().unwrap_or("(not set)"));
    println!("App secret:   {}", cfg.app_secret.as_ref().map(|_| "****").unwrap_or("(not set)"));
    println!("Redirect URI: {}", cfg.redirect_uri);
    match cfg.token_expiry {
        None => println!("Tokens:       (none)"),
        Some(exp) if cfg.is_token_expired() => println!("Tokens:       expired at {}", exp),
        Some(exp) => println!("Tokens:       valid until {}", exp),
    }
    Ok(())
}
