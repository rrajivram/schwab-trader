use anyhow::{bail, Context, Result};
use calamine::{open_workbook_from_rs, Data, DataType, Reader, Xlsx};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::io::Cursor;
use std::path::PathBuf;

use super::{HoldingsProvider, RegistryEntry};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Holding {
    pub symbol: String,
    /// Fraction of the index/fund, e.g. 0.071 = 7.1%.
    pub weight: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CachedHoldings {
    pub index_name: String,
    /// When this app fetched/cached the data.
    pub as_of: DateTime<Utc>,
    /// The feed's own "as of" date string, if the provider includes one.
    pub source_as_of: Option<String>,
    pub constituents: Vec<Holding>,
    /// Set (and the cache still served) when a live refresh failed and this
    /// is stale data — never silently hide a failed refresh.
    pub fetch_error: Option<String>,
}

const CACHE_TTL_HOURS: i64 = 24;

fn cache_path(index_name: &str) -> Result<PathBuf> {
    let dir = dirs::config_dir()
        .context("Cannot locate config directory")?
        .join("schwab-cli")
        .join("holdings");
    std::fs::create_dir_all(&dir)?;
    Ok(dir.join(format!("{}.json", index_name.to_uppercase())))
}

fn load_cache(index_name: &str) -> Option<CachedHoldings> {
    let p = cache_path(index_name).ok()?;
    let data = std::fs::read_to_string(p).ok()?;
    serde_json::from_str(&data).ok()
}

fn save_cache(cached: &CachedHoldings) -> Result<()> {
    std::fs::write(cache_path(&cached.index_name)?, serde_json::to_string_pretty(cached)?)?;
    Ok(())
}

/// Get holdings for a registry entry: serve a fresh-enough cache as-is,
/// otherwise attempt a live fetch, falling back to a stale cache (with
/// `fetch_error` set) if the live fetch fails. Only errors if there is
/// neither a usable cache nor a successful live fetch.
pub async fn get_holdings(entry: &RegistryEntry) -> Result<CachedHoldings> {
    if let Some(cached) = load_cache(&entry.name) {
        let age = Utc::now() - cached.as_of;
        if age < chrono::Duration::hours(CACHE_TTL_HOURS) && cached.fetch_error.is_none() {
            return Ok(cached);
        }
    }

    match fetch_live(entry).await {
        Ok(fresh) => {
            let _ = save_cache(&fresh);
            Ok(fresh)
        }
        Err(e) => {
            if let Some(mut stale) = load_cache(&entry.name) {
                stale.fetch_error = Some(e.to_string());
                return Ok(stale);
            }
            bail!(
                "Could not fetch holdings for {} ({}): {e}\n\
                 Check the registry entry's feed_url — the provider's file format or URL may have changed.",
                entry.name,
                entry.label
            );
        }
    }
}

async fn fetch_live(entry: &RegistryEntry) -> Result<CachedHoldings> {
    let bytes = reqwest::Client::new()
        .get(&entry.feed_url)
        .header("User-Agent", "Mozilla/5.0")
        .send()
        .await?
        .error_for_status()?
        .bytes()
        .await?;

    let (constituents, source_as_of) = match entry.provider {
        HoldingsProvider::Ssga => parse_ssga_xlsx(&bytes)?,
        HoldingsProvider::IShares | HoldingsProvider::Invesco | HoldingsProvider::Vanguard => bail!(
            "{:?} holdings feeds are not supported yet — this provider's endpoint returned an \
             unexpected response (bot-protection block or an unverifiable format) during \
             development and needs a working fetch strategy before this registry entry is usable.",
            entry.provider
        ),
    };

    if constituents.is_empty() {
        bail!("parsed 0 holdings from {} — feed layout may have changed", entry.feed_url);
    }

    Ok(CachedHoldings {
        index_name: entry.name.clone(),
        as_of: Utc::now(),
        source_as_of,
        constituents,
        fetch_error: None,
    })
}

/// Parse an SSGA/SPDR daily holdings xlsx. Verified layout (SPY/DIA/XLB, all
/// identical): 3 metadata rows, a blank row, a header row with "Ticker" and
/// "Weight" columns (located by name, not position, since column order isn't
/// guaranteed to be stable), then one row per holding until a row with
/// neither a ticker nor a weight (blank padding, or legal-disclaimer text
/// that only fills the Name column). Cash lines (ticker "-") and futures/
/// overlay lines (weight <= 0) are dropped.
fn parse_ssga_xlsx(bytes: &[u8]) -> Result<(Vec<Holding>, Option<String>)> {
    let cursor = Cursor::new(bytes.to_vec());
    let mut workbook: Xlsx<_> =
        open_workbook_from_rs(cursor).context("response was not a valid xlsx file")?;
    let sheet_name = workbook
        .sheet_names()
        .into_iter()
        .next()
        .context("xlsx has no sheets")?;
    let range = workbook
        .worksheet_range(&sheet_name)
        .map_err(|e| anyhow::anyhow!("could not read xlsx sheet '{sheet_name}': {e:?}"))?;

    let mut source_as_of = None;
    let mut header_row = None;
    let mut ticker_col = 1;
    let mut weight_col = 4;

    for (i, row) in range.rows().enumerate() {
        if row.first().map(|c| c.to_string()).as_deref() == Some("Holdings:") {
            source_as_of = row.get(1).map(|c| c.to_string());
        }
        let tc = row.iter().position(|c| c.get_string() == Some("Ticker"));
        let wc = row.iter().position(|c| c.get_string() == Some("Weight"));
        if let (Some(tc), Some(wc)) = (tc, wc) {
            ticker_col = tc;
            weight_col = wc;
            header_row = Some(i);
            break;
        }
    }
    let header_row = header_row.context("could not find a header row with Ticker/Weight columns")?;

    let mut holdings = Vec::new();
    for row in range.rows().skip(header_row + 1) {
        let ticker = row.get(ticker_col).map(|c: &Data| c.to_string()).unwrap_or_default();
        let ticker = ticker.trim();
        let weight = row.get(weight_col).and_then(DataType::as_f64);

        if ticker.is_empty() && weight.is_none() {
            break; // ran off the holdings table into blank/disclaimer rows
        }
        let Some(weight) = weight else { continue };
        if ticker.is_empty() || ticker == "-" { continue; } // cash line
        if weight <= 0.0 { continue; } // futures/derivative overlay line

        holdings.push(Holding { symbol: ticker.to_uppercase(), weight: weight / 100.0 });
    }

    Ok((holdings, source_as_of))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_real_xlb_fixture_dropping_cash_and_futures_lines() {
        let bytes = std::fs::read("tests/fixtures/holdings-daily-us-en-xlb.xlsx").unwrap();
        let (holdings, source_as_of) = parse_ssga_xlsx(&bytes).unwrap();
        assert_eq!(holdings.len(), 26, "Materials Select Sector has 26 constituents");
        assert!(source_as_of.is_some());
        assert!(holdings.iter().any(|h| h.symbol == "LIN"));
        assert!(!holdings.iter().any(|h| h.symbol == "-"));
        let total: f64 = holdings.iter().map(|h| h.weight).sum();
        assert!((0.95..=1.0).contains(&total), "weights should sum close to 1.0, got {total}");
    }

    #[test]
    fn parses_real_spy_fixture_and_skips_trailing_legal_disclaimer_text() {
        let bytes = std::fs::read("tests/fixtures/holdings-daily-us-en-spy.xlsx").unwrap();
        let (holdings, _) = parse_ssga_xlsx(&bytes).unwrap();
        assert!(holdings.len() > 400, "S&P 500 should have 400+ real holdings, got {}", holdings.len());
        assert!(holdings.iter().any(|h| h.symbol == "AAPL"));
        assert!(holdings.iter().any(|h| h.symbol == "NVDA"));
    }

    /// Hits the real SSGA endpoint through the actual binary's fetch path
    /// (not a saved fixture). Network-dependent, so it's excluded from the
    /// default `cargo test` run — run explicitly with `cargo test -- --ignored`.
    #[tokio::test]
    #[ignore]
    async fn live_fetch_xlb_resolves_and_parses() {
        let entry = RegistryEntry {
            name: "XLB".into(),
            aliases: vec!["SIXB".into()],
            label: "Materials Select Sector".into(),
            tracking_etf: "XLB".into(),
            provider: HoldingsProvider::Ssga,
            feed_url: "https://www.ssga.com/us/en/individual/library-content/products/fund-data/etfs/us/holdings-daily-us-en-xlb.xlsx".into(),
        };
        let holdings = get_holdings(&entry).await.unwrap();
        assert!(!holdings.constituents.is_empty());
        assert!(holdings.fetch_error.is_none());
    }
}
