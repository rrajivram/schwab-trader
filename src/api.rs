use anyhow::{bail, Result};
use serde_json::Value;
use std::collections::HashMap;

use crate::auth::get_valid_token;
use crate::stream::QuoteUpdate;

const QUOTES_URL: &str = "https://api.schwabapi.com/marketdata/v1/quotes";

pub async fn get_quote(symbol: &str) -> Result<()> {
    let token = get_valid_token().await?;
    let symbol_upper = symbol.to_uppercase();

    let client = reqwest::Client::new();
    let resp = client
        .get(QUOTES_URL)
        .header("Authorization", format!("Bearer {}", token))
        .query(&[("symbols", symbol_upper.as_str()), ("indicative", "false")])
        .send()
        .await?;

    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await?;
        bail!("Quote request failed ({}): {}", status, body);
    }

    let data: HashMap<String, Value> = resp.json().await?;

    match data.get(&symbol_upper) {
        Some(entry) => print_quote(&symbol_upper, entry),
        None => {
            eprintln!("No data returned for {}", symbol_upper);
            println!("{}", serde_json::to_string_pretty(&data)?);
        }
    }

    Ok(())
}

/// Fetch quote + fundamental + reference data for a batch of symbols via REST.
/// Returns QuoteUpdate structs with named fields — no unreliable stream field numbers.
pub async fn fetch_bulk_quotes(token: &str, symbols: &[String]) -> Result<Vec<QuoteUpdate>> {
    if symbols.is_empty() { return Ok(Vec::new()); }

    let resp = reqwest::Client::new()
        .get(QUOTES_URL)
        .header("Authorization", format!("Bearer {}", token))
        .query(&[
            ("symbols", symbols.join(",").as_str()),
            ("fields", "quote,fundamental,reference"),
            ("indicative", "false"),
        ])
        .send()
        .await?;

    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        bail!("bulk quotes failed ({}): {}", status, body);
    }

    let data: HashMap<String, Value> = resp.json().await?;
    let mut updates = Vec::new();

    for (sym, entry) in data {
        let q = &entry["quote"];
        let f = &entry["fundamental"];
        let r = &entry["reference"];
        let fv = |obj: &Value, k: &str| obj[k].as_f64();

        updates.push(QuoteUpdate {
            symbol:      sym,
            prev_close:  fv(q, "closePrice"),
            net_change:  fv(q, "netChange"),
            w52_high:    fv(q, "52WeekHigh"),
            w52_low:     fv(q, "52WeekLow"),
            open:        fv(q, "openPrice"),
            description: r["description"].as_str().map(str::to_string),
            pe_ratio:    fv(f, "peRatio"),
            div_amount:  fv(f, "divAmount"),
            div_yield:   fv(f, "divYield"),
            eps:         fv(f, "eps"),
            market_cap:  fv(f, "marketCap"),
            // streaming fields left to the WebSocket
            bid: None, ask: None, last: None,
            volume: None, high: None, low: None,
        });
    }

    Ok(updates)
}

/// Schwab's quotes endpoint rejects requests over this many symbols
/// ("Search combination should not exceed 500") — verified live against a
/// 504-symbol S&P 500 request, which failed until chunked.
const MAX_SYMBOLS_PER_QUOTE_REQUEST: usize = 500;

/// Fetch a current-price snapshot for a batch of symbols (for one-shot
/// planning math, not live display — `fetch_bulk_quotes` deliberately leaves
/// last/bid/ask unpopulated and defers to the WebSocket for those). Falls
/// back to the previous close if `lastPrice` is missing or zero (e.g.
/// outside market hours, or a thinly-traded symbol). Chunks large symbol
/// lists to stay under Schwab's per-request limit.
pub async fn fetch_last_prices(token: &str, symbols: &[String]) -> Result<HashMap<String, f64>> {
    let mut prices = HashMap::new();
    for chunk in symbols.chunks(MAX_SYMBOLS_PER_QUOTE_REQUEST) {
        prices.extend(fetch_last_prices_chunk(token, chunk).await?);
    }
    Ok(prices)
}

async fn fetch_last_prices_chunk(token: &str, symbols: &[String]) -> Result<HashMap<String, f64>> {
    if symbols.is_empty() { return Ok(HashMap::new()); }

    let resp = reqwest::Client::new()
        .get(QUOTES_URL)
        .header("Authorization", format!("Bearer {}", token))
        .query(&[
            ("symbols", symbols.join(",").as_str()),
            ("fields", "quote"),
            ("indicative", "false"),
        ])
        .send()
        .await?;

    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        bail!("last-price quotes failed ({}): {}", status, body);
    }

    let data: HashMap<String, Value> = resp.json().await?;
    let mut prices = HashMap::new();
    for (sym, entry) in data {
        let q = &entry["quote"];
        let price = q["lastPrice"]
            .as_f64()
            .filter(|&p| p > 0.0)
            .or_else(|| q["closePrice"].as_f64());
        if let Some(p) = price {
            prices.insert(sym, p);
        }
    }
    Ok(prices)
}

fn print_quote(symbol: &str, entry: &Value) {
    let q = match entry.get("quote") {
        Some(q) => q,
        None => {
            println!("{}", serde_json::to_string_pretty(entry).unwrap_or_default());
            return;
        }
    };

    let f64_field = |key: &str| q.get(key).and_then(|v| v.as_f64());
    let u64_field = |key: &str| q.get(key).and_then(|v| v.as_u64());

    println!("Symbol:     {}", symbol);

    if let Some(v) = f64_field("lastPrice") {
        println!("Last:       ${:.4}", v);
    }
    if let (Some(bid), Some(ask)) = (f64_field("bidPrice"), f64_field("askPrice")) {
        println!("Bid / Ask:  ${:.4} / ${:.4}", bid, ask);
    }
    if let Some(v) = f64_field("openPrice") {
        println!("Open:       ${:.4}", v);
    }
    if let (Some(lo), Some(hi)) = (f64_field("lowPrice"), f64_field("highPrice")) {
        println!("Low / High: ${:.4} / ${:.4}", lo, hi);
    }
    if let Some(v) = f64_field("closePrice") {
        println!("Prev Close: ${:.4}", v);
    }
    if let (Some(chg), Some(pct)) = (f64_field("netChange"), f64_field("netPercentChange")) {
        let sign = if chg >= 0.0 { "+" } else { "" };
        println!("Change:     {}{:.4} ({}{:.2}%)", sign, chg, sign, pct);
    }
    if let Some(v) = u64_field("totalVolume") {
        println!("Volume:     {}", v);
    }
    if let (Some(lo52), Some(hi52)) = (f64_field("52WeekLow"), f64_field("52WeekHigh")) {
        println!("52W Range:  ${:.4} - ${:.4}", lo52, hi52);
    }
}
