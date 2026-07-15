use anyhow::{Context, Result};
use std::path::PathBuf;

fn path() -> Result<PathBuf> {
    let dir = dirs::config_dir()
        .context("Cannot locate config directory")?
        .join("schwab-cli");
    std::fs::create_dir_all(&dir)?;
    Ok(dir.join("blacklist.json"))
}

pub fn load() -> Result<Vec<String>> {
    let p = path()?;
    if !p.exists() {
        return Ok(Vec::new());
    }
    let data = std::fs::read_to_string(&p)?;
    Ok(serde_json::from_str(&data)?)
}

pub fn save(symbols: &[String]) -> Result<()> {
    std::fs::write(path()?, serde_json::to_string_pretty(symbols)?)?;
    Ok(())
}
