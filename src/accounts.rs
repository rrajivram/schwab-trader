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
    /// Average purchase price as Schwab reports it. For bonds this is a
    /// percent of face value (99.58 = 99.58% of par).
    pub average_price: f64,
    /// Fixed income only (verified live on Treasury bills and notes):
    /// `instrument.maturityDate`, and `instrument.variableRate`, which holds
    /// the coupon rate in percent (0 for bills).
    pub cusip: Option<String>,
    pub maturity_date: Option<chrono::NaiveDate>,
    pub coupon_rate: Option<f64>,
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

/// "···1343": enough to tell accounts apart without showing the full number.
pub fn masked(number: &str) -> String {
    format!("···{}", &number[number.len().saturating_sub(4)..])
}

/// The one account whose number ends in `suffix`. An error says why not,
/// naming what is there, so a wrong setting is easy to fix.
pub fn by_suffix<'a>(accounts: &'a [Account], suffix: &str) -> std::result::Result<&'a Account, String> {
    let found: Vec<&Account> = accounts.iter().filter(|a| a.account_number.ends_with(suffix)).collect();
    let have = || accounts.iter().map(|a| masked(&a.account_number)).collect::<Vec<_>>().join(", ");
    match found.as_slice() {
        [one] => Ok(one),
        [] => Err(format!("No account ending in {suffix}. Accounts available: {}", have())),
        _ => Err(format!("More than one account ends in {suffix} ({}); use more digits.", have())),
    }
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
        average_price: p["averagePrice"].as_f64().unwrap_or(0.0),
        cusip: p["instrument"]["cusip"].as_str().map(str::to_string),
        // "2026-11-05T05:00:00.000+00:00" — the date part is the maturity date.
        maturity_date: p["instrument"]["maturityDate"]
            .as_str()
            .and_then(|s| chrono::NaiveDate::parse_from_str(s.get(..10)?, "%Y-%m-%d").ok()),
        coupon_rate: p["instrument"]["variableRate"].as_f64(),
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

#[cfg(test)]
mod tests {
    use super::*;

    fn acct(n: &str) -> Account {
        Account { account_number: n.into(), hash_value: format!("h{n}"), account_type: "CASH".into(), cash_balance: 0.0, liquidation_value: 0.0, positions: vec![] }
    }

    #[test]
    fn picks_the_account_by_its_last_digits() {
        let all = [acct("11112222"), acct("33334343"), acct("55556666")];
        assert_eq!(by_suffix(&all, "343").unwrap().account_number, "33334343");
        let err = by_suffix(&all, "999").unwrap_err();
        assert!(err.contains("No account ending in 999") && err.contains("···2222"), "{err}");
        let dup = [acct("1343"), acct("2343")];
        assert!(by_suffix(&dup, "343").unwrap_err().contains("More than one"));
        assert_eq!(by_suffix(&dup, "1343").unwrap().account_number, "1343");
    }

    #[test]
    fn masks_all_but_the_last_four() {
        assert_eq!(masked("12345678"), "···5678");
        assert_eq!(masked("12"), "···12");
    }
}
