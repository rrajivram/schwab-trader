use anyhow::{bail, Result};
use serde_json::Value;
use std::collections::HashMap;

use crate::auth::get_valid_token;
use crate::stream::QuoteUpdate;

const QUOTES_URL: &str = "https://api.schwabapi.com/marketdata/v1/quotes";
const INSTRUMENTS_URL: &str = "https://api.schwabapi.com/marketdata/v1/instruments";

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

/// 13-week T-bill index. Schwab quotes it as the yield ×10 (40.37 = 4.037%).
const RISK_FREE_SYMBOL: &str = "$IRX";

/// Annualized risk-free rate as a decimal (0.0404 = 4.04%), from the live
/// 13-week T-bill yield.
pub async fn fetch_risk_free_rate(token: &str) -> Result<f64> {
    let prices = fetch_last_prices(token, &[RISK_FREE_SYMBOL.to_string()]).await?;
    match prices.get(RISK_FREE_SYMBOL) {
        Some(&quoted) => irx_to_rate(quoted),
        None => bail!("no quote for {RISK_FREE_SYMBOL}"),
    }
}

fn irx_to_rate(quoted: f64) -> Result<f64> {
    let rate = quoted / 1000.0;
    // Guards against Schwab ever changing the ×10 convention.
    if !(0.0..0.25).contains(&rate) {
        bail!("{RISK_FREE_SYMBOL} quote {quoted} doesn't look like a yield ×10");
    }
    Ok(rate)
}

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

/// Per-symbol quote + fundamentals for the indexer's S&P 500 table.
#[derive(Debug, Clone, Default)]
pub struct MarketData {
    pub description: Option<String>,
    /// `lastPrice`, falling back to `closePrice` when zero/missing.
    pub price: Option<f64>,
    pub net_percent_change: Option<f64>,
    pub w52_high: Option<f64>,
    pub w52_low: Option<f64>,
    pub eps: Option<f64>,
    pub pe_ratio: Option<f64>,
    /// Percent, as Schwab reports it (1.25 = 1.25%).
    pub div_yield: Option<f64>,
    /// Annual dividend per share.
    pub div_amount: Option<f64>,
    /// Payments per year (4 = quarterly).
    pub div_freq: Option<u32>,
    pub volume: Option<u64>,
    pub avg_volume_10d: Option<f64>,
    /// Schwab's published beta vs. the S&P 500 (SPY ≈ 1.0). Only the
    /// instruments endpoint has it — the quotes `fundamental` block doesn't.
    pub beta: Option<f64>,
}

/// Fetch `MarketData` for any number of symbols, chunked under Schwab's
/// per-request limit. Symbols Schwab doesn't recognize are simply absent.
/// Results are keyed by the symbols as passed in: dotted class tickers
/// (`BRK.B`) are requested in Schwab's slash form (`BRK/B`) and mapped back.
pub async fn fetch_market_data(token: &str, symbols: &[String]) -> Result<HashMap<String, MarketData>> {
    let schwab_form: HashMap<String, &String> = symbols.iter().map(|s| (s.replace('.', "/"), s)).collect();
    let requested: Vec<String> = schwab_form.keys().cloned().collect();

    let mut out = HashMap::new();
    for chunk in requested.chunks(MAX_SYMBOLS_PER_QUOTE_REQUEST) {
        let resp = reqwest::Client::new()
            .get(QUOTES_URL)
            .header("Authorization", format!("Bearer {}", token))
            .query(&[
                ("symbols", chunk.join(",").as_str()),
                ("fields", "quote,fundamental,reference"),
                ("indicative", "false"),
            ])
            .send()
            .await?;

        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            bail!("market data quotes failed ({}): {}", status, body);
        }

        let data: HashMap<String, Value> = resp.json().await?;
        for (sym, entry) in data {
            // Unknown symbols come back under an "errors" key, not per-symbol.
            if entry.get("quote").is_none() {
                continue;
            }
            let key = schwab_form.get(&sym).map(|s| s.to_string()).unwrap_or(sym);
            out.insert(key, parse_market_data(&entry));
        }
    }

    // Beta is a nice-to-have: don't lose the whole table if this call fails.
    match fetch_betas(token, &requested).await {
        Ok(betas) => {
            for (sym, beta) in betas {
                let key = schwab_form.get(&sym).map(|s| s.to_string()).unwrap_or(sym);
                if let Some(md) = out.get_mut(&key) {
                    md.beta = Some(beta);
                }
            }
        }
        Err(e) => eprintln!("beta fetch failed: {e}"),
    }
    Ok(out)
}

/// Beta per symbol from `instruments?projection=fundamental`, which takes a
/// comma-separated batch and answers in Schwab's slash form (`BRK/B`).
/// Schwab sends exactly 0 when it has no beta (seen live on SNDK, a 2025
/// spin-off), so 0 is treated as unknown.
async fn fetch_betas(token: &str, symbols: &[String]) -> Result<HashMap<String, f64>> {
    let mut out = HashMap::new();
    for chunk in symbols.chunks(MAX_SYMBOLS_PER_QUOTE_REQUEST) {
        let resp = reqwest::Client::new()
            .get(INSTRUMENTS_URL)
            .header("Authorization", format!("Bearer {}", token))
            .query(&[("symbol", chunk.join(",").as_str()), ("projection", "fundamental")])
            .send()
            .await?;

        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            bail!("instruments fundamentals failed ({}): {}", status, body);
        }

        out.extend(parse_betas(&resp.json().await?));
    }
    Ok(out)
}

fn parse_betas(v: &Value) -> HashMap<String, f64> {
    v["instruments"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|i| Some((i["symbol"].as_str()?.to_string(), i["fundamental"]["beta"].as_f64().filter(|&b| b != 0.0)?)))
        .collect()
}

fn parse_market_data(entry: &Value) -> MarketData {
    let q = &entry["quote"];
    let f = &entry["fundamental"];
    let r = &entry["reference"];
    let fv = |obj: &Value, k: &str| obj[k].as_f64();

    MarketData {
        description: r["description"].as_str().map(str::to_string),
        price: fv(q, "lastPrice").filter(|&p| p > 0.0).or_else(|| fv(q, "closePrice")),
        net_percent_change: fv(q, "netPercentChange"),
        w52_high: fv(q, "52WeekHigh"),
        w52_low: fv(q, "52WeekLow"),
        eps: fv(f, "eps"),
        pe_ratio: fv(f, "peRatio"),
        div_yield: fv(f, "divYield"),
        div_amount: fv(f, "divAmount"),
        div_freq: f["divFreq"].as_u64().map(|v| v as u32),
        volume: q["totalVolume"].as_u64(),
        avg_volume_10d: fv(f, "avg10DaysVolume"),
        beta: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parses_betas_from_live_shape() {
        let v = json!({"instruments": [
            {"assetType": "EQUITY", "symbol": "AAPL", "fundamental": {"beta": 1.08524, "peRatio": 38.18829}},
            {"assetType": "EQUITY", "symbol": "BRK/B", "fundamental": {"beta": 0.60253}},
            {"assetType": "EQUITY", "symbol": "NOBETA", "fundamental": {"peRatio": 10.0}},
            {"assetType": "EQUITY", "symbol": "SNDK", "fundamental": {"beta": 0.0}},
        ]});
        let b = parse_betas(&v);
        assert_eq!(b.get("AAPL"), Some(&1.08524));
        assert_eq!(b.get("BRK/B"), Some(&0.60253));
        assert!(!b.contains_key("NOBETA"));
        assert!(!b.contains_key("SNDK"));
    }

    #[test]
    fn irx_quote_is_yield_times_ten() {
        assert!((irx_to_rate(40.37).unwrap() - 0.04037).abs() < 1e-12);
        assert!(irx_to_rate(4.037).is_ok()); // 0.4%: low but plausible
        assert!(irx_to_rate(403.7).is_err());
        assert!(irx_to_rate(-1.0).is_err());
    }

    #[test]
    fn empty_instruments_reply_has_no_betas() {
        assert!(parse_betas(&json!({})).is_empty());
    }
}
