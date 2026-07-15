use anyhow::{bail, Result};
use serde::Deserialize;
use serde_json::Value;

use crate::auth::get_valid_token;

const ACCOUNTS_URL: &str = "https://api.schwabapi.com/trader/v1/accounts";

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AccountNumberHash {
    pub account_number: String,
    pub hash_value: String,
}

/// Field-by-field use starts in Phase 5 (harvest.rs); today only `positions.len()`
/// is read by the AccountSelect screen.
#[allow(dead_code)]
#[derive(Debug, Clone)]
pub struct Position {
    pub symbol: String,
    pub asset_type: String,
    pub description: Option<String>,
    pub long_quantity: f64,
    pub short_quantity: f64,
    /// Schwab's `taxLotAverageLongPrice` (matched `averagePrice` in every
    /// account sampled live). A third field, `averageLongPrice`, was also
    /// present and sometimes differs slightly — it's unclear whether that one
    /// is wash-sale-adjusted. Not used here; needs confirming before Phase 5
    /// harvest logic picks one over the other.
    pub cost_basis_per_share: f64,
    pub market_value: f64,
    /// Total unrealized P&L on the long position, straight from Schwab.
    /// Negative means an unrealized loss — this is what Phase 5's loss scan
    /// will key off of instead of recomputing (price - cost) * quantity itself.
    pub long_open_profit_loss: f64,
}

#[derive(Debug, Clone)]
pub struct Account {
    pub account_number: String,
    pub hash_value: String,
    pub account_type: String,
    pub cash_balance: f64,
    pub liquidation_value: f64,
    pub positions: Vec<Position>,
}

/// List linked accounts (account number + the encrypted hash Schwab requires
/// in place of the plain account number on every other accounts/orders endpoint).
pub async fn list_account_numbers() -> Result<Vec<AccountNumberHash>> {
    let token = get_valid_token().await?;
    let resp = reqwest::Client::new()
        .get(format!("{ACCOUNTS_URL}/accountNumbers"))
        .header("Authorization", format!("Bearer {token}"))
        .send()
        .await?;

    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        bail!("accountNumbers request failed ({status}): {body}");
    }

    Ok(resp.json().await?)
}

/// Fetch one account's detail + positions.
///
/// Extraction is defensive (`Value` + `.get()/.as_f64()`) rather than a single
/// strict struct because `currentBalances`' field set differs meaningfully
/// between account types — verified live: MARGIN accounts have `buyingPower`
/// and `availableFunds`, CASH accounts have `cashAvailableForTrading` and
/// `totalCash` instead, with no overlap on those. Only `cashBalance` and
/// `liquidationValue` were confirmed present on every account type sampled.
/// `positions` is entirely absent (not an empty array) on accounts with no
/// holdings, hence the `unwrap_or_default()`.
pub async fn get_account(hash: &AccountNumberHash) -> Result<Account> {
    let token = get_valid_token().await?;
    let resp = reqwest::Client::new()
        .get(format!("{ACCOUNTS_URL}/{}", hash.hash_value))
        .header("Authorization", format!("Bearer {token}"))
        .query(&[("fields", "positions")])
        .send()
        .await?;

    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        bail!("account detail request failed ({status}): {body}");
    }

    let v: Value = resp.json().await?;
    let sa = &v["securitiesAccount"];
    let cb = &sa["currentBalances"];

    let positions = sa["positions"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .filter_map(parse_position)
        .collect();

    Ok(Account {
        account_number: hash.account_number.clone(),
        hash_value: hash.hash_value.clone(),
        account_type: sa["type"].as_str().unwrap_or("UNKNOWN").to_string(),
        cash_balance: cb["cashBalance"].as_f64().unwrap_or(0.0),
        liquidation_value: cb["liquidationValue"].as_f64().unwrap_or(0.0),
        positions,
    })
}

fn parse_position(p: &Value) -> Option<Position> {
    let symbol = p["instrument"]["symbol"].as_str()?.to_string();
    Some(Position {
        symbol,
        asset_type: p["instrument"]["assetType"].as_str().unwrap_or("UNKNOWN").to_string(),
        description: p["instrument"]["description"].as_str().map(str::to_string),
        long_quantity: p["longQuantity"].as_f64().unwrap_or(0.0),
        short_quantity: p["shortQuantity"].as_f64().unwrap_or(0.0),
        cost_basis_per_share: p["taxLotAverageLongPrice"].as_f64().unwrap_or(0.0),
        market_value: p["marketValue"].as_f64().unwrap_or(0.0),
        long_open_profit_loss: p["longOpenProfitLoss"].as_f64().unwrap_or(0.0),
    })
}

/// Fetch accountNumbers, then every account's detail. Best-effort per account:
/// one failing (e.g. a closed/restricted linked account) doesn't hide the rest.
pub async fn list_accounts() -> Result<Vec<Account>> {
    let hashes = list_account_numbers().await?;
    let mut accounts = Vec::with_capacity(hashes.len());
    for hash in &hashes {
        if let Ok(acct) = get_account(hash).await {
            accounts.push(acct);
        }
    }
    Ok(accounts)
}
