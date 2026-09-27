//! Alpha Vantage company overview: forward P/E, analyst target price and
//! ratings, and a readable company name — none of which Schwab's API has.
//!
//! The free tier allows 25 requests a day (one per symbol), so results are
//! cached on disk for `CACHE_DAYS` and a per-day request counter keeps the
//! app under the limit. A rate-limit reply from Alpha Vantage (an
//! "Information"/"Note" field instead of data) also stops fetching for the day.

use anyhow::{bail, Context, Result};
use chrono::{DateTime, Local, NaiveDate, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;
use std::path::PathBuf;

const OVERVIEW_URL: &str = "https://www.alphavantage.co/query";
pub const DAILY_LIMIT: u32 = 25;
const CACHE_DAYS: i64 = 7;

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct Overview {
    pub name: Option<String>,
    pub forward_pe: Option<f64>,
    pub target_price: Option<f64>,
    pub strong_buy: u32,
    pub buy: u32,
    pub hold: u32,
    pub sell: u32,
    pub strong_sell: u32,
    pub fetched_at: DateTime<Utc>,
}

impl Overview {
    pub fn analyst_count(&self) -> u32 {
        self.strong_buy + self.buy + self.hold + self.sell + self.strong_sell
    }

    /// Share of analysts rating it Buy or Strong Buy, if any rate it.
    pub fn buy_share(&self) -> Option<f64> {
        let n = self.analyst_count();
        (n > 0).then(|| (self.strong_buy + self.buy) as f64 / n as f64)
    }

    pub fn is_fresh(&self) -> bool {
        Utc::now() - self.fetched_at < chrono::Duration::days(CACHE_DAYS)
    }
}

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct Cache {
    pub overviews: HashMap<String, Overview>,
    /// Requests made on `usage_date` (local date — the limit resets daily).
    pub usage_date: Option<NaiveDate>,
    pub usage_count: u32,
    /// Set when Alpha Vantage itself said the daily limit was hit.
    pub limited_on: Option<NaiveDate>,
}

impl Cache {
    fn path() -> Result<PathBuf> {
        let dir = dirs::config_dir().context("Cannot locate config directory")?.join("schwab-cli");
        std::fs::create_dir_all(&dir)?;
        Ok(dir.join("alphavantage-cache.json"))
    }

    pub fn load() -> Cache {
        Self::path()
            .ok()
            .and_then(|p| std::fs::read_to_string(p).ok())
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default()
    }

    pub fn save(&self) -> Result<()> {
        std::fs::write(Self::path()?, serde_json::to_string_pretty(self)?)?;
        Ok(())
    }

    pub fn used_today(&self) -> u32 {
        if self.usage_date == Some(Local::now().date_naive()) { self.usage_count } else { 0 }
    }

    pub fn remaining_today(&self) -> u32 {
        if self.limited_on == Some(Local::now().date_naive()) {
            return 0;
        }
        DAILY_LIMIT.saturating_sub(self.used_today())
    }

    fn count_request(&mut self) {
        let today = Local::now().date_naive();
        if self.usage_date != Some(today) {
            self.usage_date = Some(today);
            self.usage_count = 0;
        }
        self.usage_count += 1;
    }
}

#[derive(Debug)]
pub enum FetchError {
    /// Alpha Vantage refused: daily (or burst) limit reached.
    RateLimited(String),
    Other(anyhow::Error),
}

/// Alpha Vantage writes dotted class tickers with a hyphen (BRK-B).
fn av_symbol(symbol: &str) -> String {
    symbol.replace(['.', '/'], "-")
}

pub async fn fetch_overview(api_key: &str, symbol: &str, cache: &mut Cache) -> std::result::Result<Overview, FetchError> {
    cache.count_request();
    let resp = reqwest::Client::new()
        .get(OVERVIEW_URL)
        .query(&[("function", "OVERVIEW"), ("symbol", av_symbol(symbol).as_str()), ("apikey", api_key)])
        .send()
        .await
        .map_err(|e| FetchError::Other(e.into()))?;
    let v: Value = resp.json().await.map_err(|e| FetchError::Other(e.into()))?;
    parse_overview(&v).map_err(|e| {
        let msg = e.to_string();
        if msg.starts_with("rate limit") {
            cache.limited_on = Some(Local::now().date_naive());
            FetchError::RateLimited(msg)
        } else {
            FetchError::Other(e)
        }
    })
}

pub fn parse_overview(v: &Value) -> Result<Overview> {
    for key in ["Information", "Note"] {
        if let Some(msg) = v.get(key).and_then(Value::as_str) {
            bail!("rate limit: {msg}");
        }
    }
    if let Some(msg) = v.get("Error Message").and_then(Value::as_str) {
        bail!("{msg}");
    }
    if v.get("Symbol").is_none() {
        bail!("no overview data for this symbol");
    }
    // Numbers arrive as strings; "None" / "-" / "0" mean unknown.
    let num = |k: &str| v.get(k).and_then(Value::as_str).and_then(|s| s.parse::<f64>().ok()).filter(|x| *x != 0.0);
    let count = |k: &str| v.get(k).and_then(Value::as_str).and_then(|s| s.parse::<u32>().ok()).unwrap_or(0);
    Ok(Overview {
        name: v.get("Name").and_then(Value::as_str).filter(|s| !s.is_empty() && *s != "None").map(str::to_string),
        forward_pe: num("ForwardPE"),
        target_price: num("AnalystTargetPrice"),
        strong_buy: count("AnalystRatingStrongBuy"),
        buy: count("AnalystRatingBuy"),
        hold: count("AnalystRatingHold"),
        sell: count("AnalystRatingSell"),
        strong_sell: count("AnalystRatingStrongSell"),
        fetched_at: Utc::now(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// Fields copied from a live KO response.
    #[test]
    fn parses_live_shape() {
        let v = json!({
            "Symbol": "KO", "Name": "Coca-Cola Co", "PERatio": "26.37", "ForwardPE": "25.32",
            "AnalystTargetPrice": "94.7", "AnalystRatingStrongBuy": "4", "AnalystRatingBuy": "13",
            "AnalystRatingHold": "5", "AnalystRatingSell": "0", "AnalystRatingStrongSell": "1"
        });
        let o = parse_overview(&v).unwrap();
        assert_eq!(o.name.as_deref(), Some("Coca-Cola Co"));
        assert_eq!(o.forward_pe, Some(25.32));
        assert_eq!(o.target_price, Some(94.7));
        assert_eq!(o.analyst_count(), 23);
        assert!((o.buy_share().unwrap() - 17.0 / 23.0).abs() < 1e-12);
    }

    #[test]
    fn missing_values_are_none() {
        let o = parse_overview(&json!({"Symbol": "X", "ForwardPE": "None", "AnalystTargetPrice": "-"})).unwrap();
        assert_eq!(o.forward_pe, None);
        assert_eq!(o.target_price, None);
        assert_eq!(o.buy_share(), None);
    }

    #[test]
    fn rate_limit_reply_is_detected() {
        let err = parse_overview(&json!({"Information": "We have detected your API key ... 25 requests per day."})).unwrap_err();
        assert!(err.to_string().starts_with("rate limit"));
    }

    #[test]
    fn class_tickers_use_hyphens() {
        assert_eq!(av_symbol("BRK.B"), "BRK-B");
        assert_eq!(av_symbol("BRK/B"), "BRK-B");
    }
}
