use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Watchlist {
    pub name: String,
    pub symbols: Vec<String>,
}

impl Watchlist {
    pub fn new(name: impl Into<String>) -> Self {
        Self { name: name.into(), symbols: Vec::new() }
    }
}

fn path() -> Result<PathBuf> {
    let dir = dirs::config_dir()
        .context("Cannot locate config directory")?
        .join("schwab-cli");
    std::fs::create_dir_all(&dir)?;
    Ok(dir.join("watchlists.json"))
}

pub fn load() -> Result<Vec<Watchlist>> {
    let p = path()?;
    if !p.exists() {
        return Ok(Vec::new());
    }
    let data = std::fs::read_to_string(&p)?;
    Ok(serde_json::from_str(&data)?)
}

pub fn save(lists: &[Watchlist]) -> Result<()> {
    std::fs::write(path()?, serde_json::to_string_pretty(lists)?)?;
    Ok(())
}
