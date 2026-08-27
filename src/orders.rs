use anyhow::{bail, Result};
use clap::ValueEnum;
use serde_json::{json, Value};

use crate::auth::get_valid_token;
use crate::rebalance::RebalancePlan;

const ORDERS_URL: &str = "https://api.schwabapi.com/trader/v1/accounts";

/// Which instrument type an order is for. `FixedIncome` (bonds, Treasuries)
/// is CONFIRMED NON-FUNCTIONAL: a live order (CUSIP 912797UJ4, oversized
/// quantity, LIMIT @ 99.584) got a 400 straight from schema validation —
/// "Valid value for `assetType` is [EQUITY, OPTION]" — not a buying-power
/// rejection. Schwab's Trader API does not accept order submissions for
/// bonds regardless of size/price; `FIXED_INCOME` only shows up when
/// *reading* existing positions. Kept here as a documented dead end rather
/// than deleted, in case a different app registration/account tier ever
/// changes this.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum AssetType {
    Equity,
    FixedIncome,
}

#[derive(Debug, Clone)]
pub enum OrderOutcome {
    /// The request body was built and returned without being sent —
    /// `dry_run = true` always takes this path. Use this for all
    /// development/testing; never assume a live order will behave the same
    /// until it's been confirmed against a real (ideally tiny) submission.
    DryRun { request_body: Value },
    /// Submitted and accepted (2xx). Schwab's order endpoint returns no body
    /// on success, just a `Location` header pointing at the new order.
    Submitted { order_location: Option<String> },
    /// Submitted and rejected by Schwab, or the request itself failed
    /// (network/auth error, `status: 0`).
    Rejected { status: u16, body: String },
}

#[derive(Debug, Clone)]
pub struct OrderResult {
    pub symbol: String,
    pub quantity: f64,
    pub outcome: OrderOutcome,
}

/// Build a single-leg BUY order. Quantity is a JSON number and can carry a
/// fractional value for equities — whether Schwab's endpoint actually
/// *accepts* a fractional quantity for a given account/symbol is
/// UNCONFIRMED. This has deliberately never been tested against a live
/// submission; that requires an explicit, separate go-ahead before it's
/// attempted, since a rejected assumption here means real (if small) money
/// movement.
///
/// `symbol_or_cusip` is a ticker for `Equity` orders, and a CUSIP for
/// `FixedIncome` orders (bonds are identified by CUSIP, not ticker).
/// `price` is required (and used as a `LIMIT` price) for `FixedIncome`
/// orders — bonds are never submitted as `MARKET` orders here — and ignored
/// for `Equity` orders, which stay `MARKET` as before.
fn build_order_body(asset_type: AssetType, symbol_or_cusip: &str, quantity: f64, price: Option<f64>) -> Result<Value> {
    let (order_type, instrument, extra_price) = match asset_type {
        AssetType::Equity => (
            "MARKET",
            json!({
                "symbol": symbol_or_cusip,
                "assetType": "EQUITY"
            }),
            None,
        ),
        AssetType::FixedIncome => {
            let Some(price) = price else {
                bail!("--price is required for fixed-income orders (bonds don't trade at MARKET)");
            };
            (
                "LIMIT",
                json!({
                    "symbol": symbol_or_cusip,
                    "cusip": symbol_or_cusip,
                    "assetType": "FIXED_INCOME"
                }),
                Some(price),
            )
        }
    };

    let mut body = json!({
        "orderType": order_type,
        "session": "NORMAL",
        "duration": "DAY",
        "orderStrategyType": "SINGLE",
        "orderLegCollection": [{
            "instruction": "BUY",
            "quantity": round_quantity(quantity),
            "instrument": instrument
        }]
    });
    if let Some(price) = extra_price {
        body["price"] = json!(price);
    }
    Ok(body)
}

/// `target_shares` is a raw float (e.g. `0.26724343162931624`) — round to 6
/// decimal places before it ever goes on the wire. Sending 17 significant
/// digits of float noise as a share quantity would almost certainly get
/// rejected even on an account/symbol where fractional orders are otherwise
/// accepted; 6 decimals matches the precision fractional-share platforms
/// typically quote to.
fn round_quantity(quantity: f64) -> f64 {
    (quantity * 1_000_000.0).round() / 1_000_000.0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_quantity_drops_float_noise_past_6_decimals() {
        assert_eq!(round_quantity(0.26724343162931624), 0.267243);
        assert_eq!(round_quantity(3.0), 3.0);
    }

    #[test]
    fn dry_run_never_sends_and_carries_the_rounded_quantity() {
        let outcome = tokio::runtime::Runtime::new()
            .unwrap()
            .block_on(submit_order("hash123", "AAPL", 0.26724343162931624, true))
            .unwrap();
        let OrderOutcome::DryRun { request_body } = outcome else { panic!("expected DryRun") };
        assert_eq!(request_body["orderLegCollection"][0]["quantity"], 0.267243);
        assert_eq!(request_body["orderLegCollection"][0]["instruction"], "BUY");
    }

    #[test]
    fn fixed_income_order_is_a_limit_order_with_cusip_and_price() {
        let outcome = tokio::runtime::Runtime::new()
            .unwrap()
            .block_on(submit_order_as("hash123", AssetType::FixedIncome, "912797FZ0", 1000.0, Some(99.5), true))
            .unwrap();
        let OrderOutcome::DryRun { request_body } = outcome else { panic!("expected DryRun") };
        assert_eq!(request_body["orderType"], "LIMIT");
        assert_eq!(request_body["price"], 99.5);
        let instrument = &request_body["orderLegCollection"][0]["instrument"];
        assert_eq!(instrument["assetType"], "FIXED_INCOME");
        assert_eq!(instrument["cusip"], "912797FZ0");
    }

    #[test]
    fn fixed_income_order_without_price_is_rejected() {
        let err = tokio::runtime::Runtime::new()
            .unwrap()
            .block_on(submit_order_as("hash123", AssetType::FixedIncome, "912797FZ0", 1000.0, None, true))
            .unwrap_err();
        assert!(err.to_string().contains("--price is required"));
    }
}

/// Submit one equity order leg (market BUY). `dry_run = true` builds and
/// returns the request body without sending anything.
pub async fn submit_order(account_hash: &str, symbol: &str, quantity: f64, dry_run: bool) -> Result<OrderOutcome> {
    submit_order_as(account_hash, AssetType::Equity, symbol, quantity, None, dry_run).await
}

/// Submit one order leg of any supported asset type. `dry_run = true`
/// builds and returns the request body without sending anything. See
/// `build_order_body` for what `price` means per asset type.
pub async fn submit_order_as(
    account_hash: &str,
    asset_type: AssetType,
    symbol_or_cusip: &str,
    quantity: f64,
    price: Option<f64>,
    dry_run: bool,
) -> Result<OrderOutcome> {
    let body = build_order_body(asset_type, symbol_or_cusip, quantity, price)?;
    if dry_run {
        return Ok(OrderOutcome::DryRun { request_body: body });
    }

    let token = get_valid_token().await?;
    let resp = reqwest::Client::new()
        .post(format!("{ORDERS_URL}/{account_hash}/orders"))
        .header("Authorization", format!("Bearer {token}"))
        .json(&body)
        .send()
        .await?;

    let status = resp.status();
    if status.is_success() {
        let order_location = resp
            .headers()
            .get("Location")
            .and_then(|v| v.to_str().ok())
            .map(str::to_string);
        Ok(OrderOutcome::Submitted { order_location })
    } else {
        let body = resp.text().await.unwrap_or_default();
        Ok(OrderOutcome::Rejected { status: status.as_u16(), body })
    }
}

/// Submit every buy line in a plan, one at a time. Never aborts the batch on
/// an individual rejection — the caller needs to see every line's outcome,
/// not a generic "something failed."
pub async fn submit_plan(plan: &RebalancePlan, dry_run: bool) -> Vec<OrderResult> {
    let mut results = Vec::with_capacity(plan.lines.len());
    for line in &plan.lines {
        let outcome = match submit_order(&plan.account_hash, &line.symbol, line.target_shares, dry_run).await {
            Ok(o) => o,
            Err(e) => OrderOutcome::Rejected { status: 0, body: e.to_string() },
        };
        results.push(OrderResult { symbol: line.symbol.clone(), quantity: line.target_shares, outcome });
    }
    results
}

/// CLI entry point for `schwab order`: resolve the account hash (explicit
/// flag, else fetched live), submit via the same `submit_order` path the
/// TUI's dry-run flow uses, and print the outcome.
///
/// Deliberately does NOT fall back to `Config::selected_account_hash` —
/// verified live that Schwab's account hash is not a stable long-lived ID:
/// a hash cached from an earlier TUI session was rejected with "Invalid
/// account number" by the orders endpoint even though it matched the same
/// account at selection time. Always re-resolving via `accountNumbers`
/// avoids submitting against a hash that's gone stale.
pub async fn place_order_cli(
    symbol: &str,
    quantity: f64,
    asset_type: AssetType,
    price: Option<f64>,
    account_hash: Option<String>,
    live: bool,
) -> Result<()> {
    let account_hash = match account_hash {
        Some(h) => h,
        None => {
            let mut accounts = crate::accounts::list_account_numbers().await?;
            match accounts.len() {
                0 => bail!("No linked accounts found."),
                1 => accounts.remove(0).hash_value,
                n => bail!("{n} linked accounts found — pass --account-hash to pick one."),
            }
        }
    };

    // CUSIPs aren't uppercased/normalized the way tickers are.
    let symbol_or_cusip = match asset_type {
        AssetType::Equity => symbol.to_uppercase(),
        AssetType::FixedIncome => symbol.to_string(),
    };
    let outcome = submit_order_as(&account_hash, asset_type, &symbol_or_cusip, quantity, price, !live).await?;
    match outcome {
        OrderOutcome::DryRun { request_body } => {
            println!("Dry run — request body that would be sent:");
            println!("{}", serde_json::to_string_pretty(&request_body)?);
        }
        OrderOutcome::Submitted { order_location } => {
            println!("Submitted.");
            match order_location {
                Some(loc) => println!("Order location: {loc}"),
                None => println!("(no Location header returned)"),
            }
        }
        OrderOutcome::Rejected { status, body } => {
            println!("Rejected (status {status}):");
            println!("{body}");
        }
    }
    Ok(())
}
