//! Weekly closing prices from Schwab's pricehistory endpoint, for fitting
//! alpha and beta and for backtesting. Closes are split-adjusted but
//! price-only (no dividends). Newer listings simply start later.
//!
//! One symbol per request, so a full S&P 500 load is ~500 requests: results
//! are cached on disk for `CACHE_DAYS`.

use anyhow::{bail, Context, Result};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;
use std::path::PathBuf;

const PRICE_HISTORY_URL: &str = "https://api.schwabapi.com/marketdata/v1/pricehistory";
/// The market every stock is regressed on.
pub const BENCHMARK: &str = "SPY";
/// Enough for a multi-year backtest; alpha only uses the latest `FIT_WEEKS`.
pub const YEARS: u32 = 10;
/// Weekly closes used for the alpha/beta fit shown in the tables (3 years).
pub const FIT_WEEKS: usize = 157;
const CACHE_DAYS: i64 = 7;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Series {
    /// (week start, epoch ms; close), oldest first.
    pub closes: Vec<(i64, f64)>,
    pub fetched_at: DateTime<Utc>,
}

impl Series {
    pub fn is_fresh(&self) -> bool {
        Utc::now() - self.fetched_at < chrono::Duration::days(CACHE_DAYS)
    }
}

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct Cache {
    pub series: HashMap<String, Series>,
}

impl Cache {
    fn path() -> Result<PathBuf> {
        let dir = dirs::config_dir().context("Cannot locate config directory")?.join("schwab-cli");
        std::fs::create_dir_all(&dir)?;
        // Named by span so the earlier 3-year cache isn't mistaken for 10.
        Ok(dir.join("price-history-10y.json"))
    }

    pub fn load() -> Cache {
        Self::path()
            .ok()
            .and_then(|p| std::fs::read_to_string(p).ok())
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default()
    }

    /// Compact JSON: ~520 points × 500 symbols, a few MB.
    pub fn save(&self) -> Result<()> {
        std::fs::write(Self::path()?, serde_json::to_string(self)?)?;
        Ok(())
    }
}

/// `YEARS` of weekly closes for one symbol (dotted class tickers are sent in
/// Schwab's slash form). Retries a couple of times on Schwab's rate limit
/// and on transient failures (a truncated body was seen live on MMM).
pub async fn fetch_weekly(token: &str, symbol: &str) -> Result<Series> {
    let years = YEARS.to_string();
    let schwab_symbol = symbol.replace('.', "/");
    let client = reqwest::Client::new();
    for attempt in 0.. {
        let sent = client
            .get(PRICE_HISTORY_URL)
            .header("Authorization", format!("Bearer {}", token))
            .query(&[
                ("symbol", schwab_symbol.as_str()),
                ("periodType", "year"),
                ("period", years.as_str()),
                ("frequencyType", "weekly"),
                ("frequency", "1"),
            ])
            .send()
            .await;
        let resp = match sent {
            Ok(r) => r,
            Err(_) if attempt < 2 => {
                tokio::time::sleep(std::time::Duration::from_secs(2)).await;
                continue;
            }
            Err(e) => return Err(e.into()),
        };

        let status = resp.status();
        if status == reqwest::StatusCode::TOO_MANY_REQUESTS && attempt < 2 {
            tokio::time::sleep(std::time::Duration::from_secs(30)).await;
            continue;
        }
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            bail!("price history for {symbol} failed ({status}): {body}");
        }
        match resp.json::<Value>().await {
            Ok(v) => return Ok(Series { closes: parse_candles(&v), fetched_at: Utc::now() }),
            Err(_) if attempt < 2 => tokio::time::sleep(std::time::Duration::from_secs(2)).await,
            Err(e) => return Err(e).with_context(|| format!("price history for {symbol}")),
        }
    }
    unreachable!()
}

/// The last `n` closes (all of them if there are fewer).
pub fn tail(closes: &[(i64, f64)], n: usize) -> &[(i64, f64)] {
    &closes[closes.len().saturating_sub(n)..]
}

fn parse_candles(v: &Value) -> Vec<(i64, f64)> {
    let mut out: Vec<(i64, f64)> = v["candles"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|c| Some((c["datetime"].as_i64()?, c["close"].as_f64()?)))
        .collect();
    out.sort_by_key(|&(d, _)| d);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parses_live_shape_oldest_first() {
        let v = json!({"symbol": "NVDA", "empty": false, "candles": [
            {"close": 239.24, "datetime": 1791176400000i64, "high": 243.37, "low": 235.15, "open": 236.09, "volume": 228873375},
            {"close": 41.387, "datetime": 1697432400000i64, "high": 46.225, "low": 41.078, "open": 45.063, "volume": 2793225380i64},
        ]});
        assert_eq!(parse_candles(&v), vec![(1697432400000, 41.387), (1791176400000, 239.24)]);
    }

    #[test]
    fn empty_reply_has_no_closes() {
        assert!(parse_candles(&json!({"candles": [], "empty": true})).is_empty());
    }
}
