//! Dividends received per symbol, from the account's transaction history.
//! Best-effort on two counts:
//! - Schwab limits each transactions request to a one-year window, so this
//!   walks back year by year up to `YEARS_BACK`.
//! - Dividend transactions carry no ticker — verified live, their only
//!   transfer item is CURRENCY_USD. The payer is identified solely by
//!   `description` (e.g. "FORD MTR CO DEL"), which matches the quote
//!   endpoint's `reference.description` for that symbol exactly, so held
//!   symbols are matched by description.

use anyhow::{bail, Result};
use chrono::{Duration, Utc};
use serde_json::Value;
use std::collections::HashMap;

use crate::api::fetch_market_data;
use crate::auth::get_valid_token;

const ACCOUNTS_URL: &str = "https://api.schwabapi.com/trader/v1/accounts";
const YEARS_BACK: i64 = 5;

/// Total dividend cash received per symbol, for the given held symbols.
/// A failing older year (e.g. before the account existed) ends the walk
/// early rather than failing the whole lookup.
pub async fn dividends_by_symbol(account_hash: &str, held_symbols: &[String]) -> Result<HashMap<String, f64>> {
    if held_symbols.is_empty() {
        return Ok(HashMap::new());
    }
    let token = get_valid_token().await?;

    let mut by_description: HashMap<String, f64> = HashMap::new();
    let now = Utc::now();
    for year in 0..YEARS_BACK {
        let end = now - Duration::days(365 * year);
        let start = end - Duration::days(365);
        let txns = match fetch_window(&token, account_hash, start, end).await {
            Ok(t) => t,
            Err(e) if year == 0 => return Err(e),
            Err(_) => break,
        };
        for t in &txns {
            if let Some((desc, amount)) = parse_dividend(t) {
                *by_description.entry(desc).or_insert(0.0) += amount;
            }
        }
    }
    if by_description.is_empty() {
        return Ok(HashMap::new());
    }

    let quotes = fetch_market_data(&token, held_symbols).await?;
    let symbol_by_description: HashMap<String, &String> = quotes
        .iter()
        .filter_map(|(sym, md)| Some((normalize(md.description.as_deref()?), sym)))
        .collect();

    let mut totals = HashMap::new();
    for (desc, amount) in by_description {
        if let Some(sym) = symbol_by_description.get(&desc) {
            *totals.entry((*sym).clone()).or_insert(0.0) += amount;
        }
    }
    Ok(totals)
}

fn normalize(description: &str) -> String {
    description.split_whitespace().collect::<Vec<_>>().join(" ").to_uppercase()
}

async fn fetch_window(
    token: &str,
    account_hash: &str,
    start: chrono::DateTime<Utc>,
    end: chrono::DateTime<Utc>,
) -> Result<Vec<Value>> {
    let fmt = |d: chrono::DateTime<Utc>| d.format("%Y-%m-%dT%H:%M:%S%.3fZ").to_string();
    let resp = reqwest::Client::new()
        .get(format!("{ACCOUNTS_URL}/{account_hash}/transactions"))
        .header("Authorization", format!("Bearer {token}"))
        .query(&[
            ("startDate", fmt(start)),
            ("endDate", fmt(end)),
            ("types", "DIVIDEND_OR_INTEREST".to_string()),
        ])
        .send()
        .await?;

    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        bail!("transactions request failed ({status}): {body}");
    }
    Ok(resp.json::<Vec<Value>>().await?)
}

/// A dividend's payer description + cash amount. Bank interest (description
/// "BANK INT ...") isn't a dividend and is skipped.
fn parse_dividend(t: &Value) -> Option<(String, f64)> {
    let amount = t["netAmount"].as_f64()?;
    let desc = normalize(t["description"].as_str()?);
    if desc.starts_with("BANK INT") {
        return None;
    }
    Some((desc, amount))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// Shape copied from a live Schwab response.
    #[test]
    fn parses_payer_description() {
        let t = json!({
            "description": "FORD MTR CO DEL",
            "type": "DIVIDEND_OR_INTEREST",
            "netAmount": 0.15,
            "transferItems": [{"instrument": {"assetType": "CURRENCY", "symbol": "CURRENCY_USD"}, "amount": 0.15}]
        });
        assert_eq!(parse_dividend(&t), Some(("FORD MTR CO DEL".to_string(), 0.15)));
    }

    #[test]
    fn bank_interest_is_ignored() {
        let t = json!({"description": "BANK INT 081626-091526 SCHWAB BANK", "netAmount": 0.04});
        assert_eq!(parse_dividend(&t), None);
    }
}
