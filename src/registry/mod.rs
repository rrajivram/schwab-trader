mod entry;
pub mod holdings;
pub use entry::{HoldingsProvider, RegistryEntry};
pub use holdings::get_holdings;

use anyhow::{Context, Result};
use std::path::PathBuf;

fn path() -> Result<PathBuf> {
    let dir = dirs::config_dir()
        .context("Cannot locate config directory")?
        .join("schwab-cli");
    std::fs::create_dir_all(&dir)?;
    Ok(dir.join("index_registry.json"))
}

/// Load the user's index registry, seeding it with baked-in defaults on first run.
pub fn load() -> Result<Vec<RegistryEntry>> {
    let p = path()?;
    if !p.exists() {
        let defaults = default_entries();
        save(&defaults)?;
        return Ok(defaults);
    }
    let data = std::fs::read_to_string(&p)?;
    Ok(serde_json::from_str(&data)?)
}

pub fn save(entries: &[RegistryEntry]) -> Result<()> {
    std::fs::write(path()?, serde_json::to_string_pretty(entries)?)?;
    Ok(())
}

/// Find a registry entry by canonical name or alias, case-insensitively.
pub fn find<'a>(entries: &'a [RegistryEntry], query: &str) -> Option<&'a RegistryEntry> {
    let q = query.trim();
    entries
        .iter()
        .find(|e| e.name.eq_ignore_ascii_case(q) || e.aliases.iter().any(|a| a.eq_ignore_ascii_case(q)))
}

/// SSGA (SPDR) publishes each fund's daily holdings at this predictable URL —
/// verified reachable via a plain GET (no cookies/session) for SPY, DIA, and
/// all 11 Select Sector SPDRs during Phase 0 research.
fn ssga_url(ticker: &str) -> String {
    format!(
        "https://www.ssga.com/us/en/individual/library-content/products/fund-data/etfs/us/holdings-daily-us-en-{}.xlsx",
        ticker.to_lowercase()
    )
}

/// Baked-in defaults, written to the user's registry file on first run so
/// they can edit/fix feed URLs later without recompiling. See HoldingsProvider
/// doc comments for which of these are verified fetchable vs. flagged risks.
fn default_entries() -> Vec<RegistryEntry> {
    let mut v = vec![
        RegistryEntry {
            name: "SPY".into(),
            aliases: vec!["S&P 500".into(), "SPX".into()],
            label: "S&P 500".into(),
            tracking_etf: "SPY".into(),
            provider: HoldingsProvider::Ssga,
            feed_url: ssga_url("SPY"),
        },
        RegistryEntry {
            name: "QQQ".into(),
            aliases: vec!["NASDAQ 100".into(), "NDX".into()],
            label: "Nasdaq 100".into(),
            tracking_etf: "QQQ".into(),
            provider: HoldingsProvider::Invesco,
            feed_url: "https://www.invesco.com/us/financial-products/etfs/holdings/main/holdings/0?audienceType=Investor&action=download&ticker=QQQ".into(),
        },
        RegistryEntry {
            name: "DIA".into(),
            aliases: vec!["DOW 30".into(), "DJIA".into()],
            label: "Dow Jones Industrial Average".into(),
            tracking_etf: "DIA".into(),
            provider: HoldingsProvider::Ssga,
            feed_url: ssga_url("DIA"),
        },
    ];

    // The 11 Select Sector SPDRs. CBOE/S&P also publish these as standalone
    // indices with their own "SIX_" tickers — only include the alias where
    // independently confirmed (SIXB/SIXE/SIXT) rather than guessing the rest.
    let sectors: &[(&str, &str, &[&str])] = &[
        ("XLB",  "Materials Select Sector",             &["SIXB"]),
        ("XLC",  "Communication Services Select Sector", &[]),
        ("XLE",  "Energy Select Sector",                 &["SIXE"]),
        ("XLF",  "Financial Select Sector",               &[]),
        ("XLI",  "Industrial Select Sector",              &[]),
        ("XLK",  "Technology Select Sector",              &["SIXT"]),
        ("XLP",  "Consumer Staples Select Sector",       &[]),
        ("XLRE", "Real Estate Select Sector",             &[]),
        ("XLU",  "Utilities Select Sector",               &[]),
        ("XLV",  "Health Care Select Sector",             &[]),
        ("XLY",  "Consumer Discretionary Select Sector", &[]),
    ];

    for (ticker, label, extra_aliases) in sectors {
        v.push(RegistryEntry {
            name: ticker.to_string(),
            aliases: extra_aliases.iter().map(|s| s.to_string()).collect(),
            label: label.to_string(),
            tracking_etf: ticker.to_string(),
            provider: HoldingsProvider::Ssga,
            feed_url: ssga_url(ticker),
        });
    }

    v
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_cover_sp500_nasdaq100_dow30_and_11_sector_spdrs() {
        let entries = default_entries();
        assert_eq!(entries.len(), 3 + 11);
        let names: Vec<&str> = entries.iter().map(|e| e.name.as_str()).collect();
        for expected in ["SPY", "QQQ", "DIA", "XLB", "XLC", "XLK"] {
            assert!(names.contains(&expected), "missing {expected}");
        }
    }

    #[test]
    fn find_resolves_sixb_alias_to_materials_sector_spdr() {
        let entries = default_entries();
        let found = find(&entries, "SIXB").expect("SIXB should resolve");
        assert_eq!(found.tracking_etf, "XLB");
        assert_eq!(found.provider, HoldingsProvider::Ssga);
    }

    #[test]
    fn find_is_case_insensitive_and_matches_canonical_name() {
        let entries = default_entries();
        assert!(find(&entries, "spy").is_some());
        assert!(find(&entries, "nope").is_none());
    }
}
