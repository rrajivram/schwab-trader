use anyhow::Result;
use chrono::{DateTime, Utc};
use std::collections::HashMap;

use crate::registry::holdings::{CachedHoldings, Holding};

#[derive(Debug, Clone)]
pub struct PlanLine {
    pub symbol: String,
    /// Post-blacklist, renormalized weight (fraction of 1.0).
    pub target_weight: f64,
    pub target_dollars: f64,
    pub last_price: f64,
    /// Fractional share count implied by `target_dollars / last_price`.
    pub target_shares: f64,
    /// `target_shares` floored — the fallback if a fractional order is
    /// rejected at submission time (Phase 4 concern; computed here for display).
    pub whole_shares_fallback: u64,
}

#[derive(Debug, Clone)]
pub struct RebalancePlan {
    pub index_name: String,
    /// Not yet read anywhere — Phase 4 (order submission) is what actually
    /// needs to know which account to place orders against.
    #[allow(dead_code)]
    pub account_hash: String,
    pub total_amount: f64,
    /// Propagated from the holdings snapshot this plan was built from.
    pub as_of_holdings: DateTime<Utc>,
    pub lines: Vec<PlanLine>,
    pub blacklisted_excluded: Vec<String>,
    /// Constituents with no fetchable price, excluded from `lines`.
    pub price_unavailable: Vec<String>,
    /// Dollars not allocated to any line (mainly `price_unavailable` symbols'
    /// share of the total; should be near zero otherwise).
    pub cash_remainder: f64,
}

/// Drop blacklisted symbols and renormalize the remaining weights to sum to
/// 1.0 relative to each other. This also absorbs any shortfall already
/// present in `holdings` — e.g. cash/derivative overlay lines dropped while
/// parsing the source file mean raw weights can sum to slightly under 1.0;
/// dividing by their own sum rescales that away in the same step.
pub fn apply_blacklist(holdings: &[Holding], blacklist: &[String]) -> (Vec<Holding>, Vec<String>) {
    let blacklist_set: std::collections::HashSet<&str> = blacklist.iter().map(String::as_str).collect();
    let mut kept = Vec::new();
    let mut excluded = Vec::new();
    for h in holdings {
        if blacklist_set.contains(h.symbol.as_str()) {
            excluded.push(h.symbol.clone());
        } else {
            kept.push(h.clone());
        }
    }

    let sum: f64 = kept.iter().map(|h| h.weight).sum();
    if sum > 0.0 {
        for h in &mut kept {
            h.weight /= sum;
        }
    }
    (kept, excluded)
}

/// Pure plan-building math: no I/O. Splits `total_amount` across
/// post-blacklist constituents by weight, using `prices` for a per-symbol
/// current price. This allocates *new* money according to index weights —
/// it does not net against what the account already holds (that would be a
/// whole-portfolio rebalance, a different, bigger feature than "invest this
/// dollar amount to match the index").
pub fn build_plan(
    index_name: &str,
    account_hash: &str,
    holdings: &CachedHoldings,
    blacklist: &[String],
    total_amount: f64,
    prices: &HashMap<String, f64>,
) -> RebalancePlan {
    let (kept, blacklisted_excluded) = apply_blacklist(&holdings.constituents, blacklist);

    let mut lines = Vec::new();
    let mut price_unavailable = Vec::new();
    let mut allocated = 0.0;

    for h in &kept {
        match prices.get(&h.symbol) {
            Some(&price) if price > 0.0 => {
                let target_dollars = total_amount * h.weight;
                let target_shares = target_dollars / price;
                lines.push(PlanLine {
                    symbol: h.symbol.clone(),
                    target_weight: h.weight,
                    target_dollars,
                    last_price: price,
                    target_shares,
                    whole_shares_fallback: target_shares.floor() as u64,
                });
                allocated += target_dollars;
            }
            _ => price_unavailable.push(h.symbol.clone()),
        }
    }

    RebalancePlan {
        index_name: index_name.to_string(),
        account_hash: account_hash.to_string(),
        total_amount,
        as_of_holdings: holdings.as_of,
        lines,
        blacklisted_excluded,
        price_unavailable,
        cash_remainder: total_amount - allocated,
    }
}

/// Fetch current prices for every constituent, then build the plan.
pub async fn plan_investment(
    index_name: &str,
    account_hash: &str,
    holdings: &CachedHoldings,
    blacklist: &[String],
    total_amount: f64,
) -> Result<RebalancePlan> {
    let symbols: Vec<String> = holdings.constituents.iter().map(|h| h.symbol.clone()).collect();
    let token = crate::auth::get_valid_token().await?;
    let prices = crate::api::fetch_last_prices(&token, &symbols).await?;
    Ok(build_plan(index_name, account_hash, holdings, blacklist, total_amount, &prices))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn holding(symbol: &str, weight: f64) -> Holding {
        Holding { symbol: symbol.to_string(), weight }
    }

    #[test]
    fn apply_blacklist_drops_and_renormalizes() {
        let holdings = vec![holding("AAPL", 0.5), holding("MSFT", 0.3), holding("XOM", 0.2)];
        let (kept, excluded) = apply_blacklist(&holdings, &["XOM".to_string()]);
        assert_eq!(excluded, vec!["XOM".to_string()]);
        assert_eq!(kept.len(), 2);
        let sum: f64 = kept.iter().map(|h| h.weight).sum();
        assert!((sum - 1.0).abs() < 1e-9);
        let aapl = kept.iter().find(|h| h.symbol == "AAPL").unwrap();
        assert!((aapl.weight - 0.625).abs() < 1e-9); // 0.5 / 0.8
    }

    #[test]
    fn apply_blacklist_absorbs_pre_existing_shortfall() {
        // Simulates holdings.rs having already dropped cash/derivative lines,
        // leaving weights that sum to 0.98 instead of 1.0.
        let holdings = vec![holding("A", 0.49), holding("B", 0.49)];
        let (kept, _) = apply_blacklist(&holdings, &[]);
        let sum: f64 = kept.iter().map(|h| h.weight).sum();
        assert!((sum - 1.0).abs() < 1e-9);
    }

    #[test]
    fn build_plan_allocates_by_weight_and_tracks_missing_prices() {
        let holdings = CachedHoldings {
            index_name: "TEST".into(),
            as_of: Utc::now(),
            source_as_of: None,
            constituents: vec![holding("AAPL", 0.6), holding("MSFT", 0.4)],
            fetch_error: None,
        };
        let mut prices = HashMap::new();
        prices.insert("AAPL".to_string(), 200.0);
        // MSFT price intentionally missing.

        let plan = build_plan("TEST", "hash123", &holdings, &[], 1000.0, &prices);

        assert_eq!(plan.lines.len(), 1);
        let aapl = &plan.lines[0];
        assert_eq!(aapl.symbol, "AAPL");
        assert!((aapl.target_dollars - 600.0).abs() < 1e-9);
        assert!((aapl.target_shares - 3.0).abs() < 1e-9);
        assert_eq!(aapl.whole_shares_fallback, 3);

        assert_eq!(plan.price_unavailable, vec!["MSFT".to_string()]);
        assert!((plan.cash_remainder - 400.0).abs() < 1e-9);
    }

    #[test]
    fn build_plan_with_full_prices_leaves_near_zero_remainder() {
        let holdings = CachedHoldings {
            index_name: "TEST".into(),
            as_of: Utc::now(),
            source_as_of: None,
            constituents: vec![holding("A", 0.5), holding("B", 0.3), holding("C", 0.2)],
            fetch_error: None,
        };
        let prices = HashMap::from([
            ("A".to_string(), 10.0),
            ("B".to_string(), 20.0),
            ("C".to_string(), 30.0),
        ]);

        let plan = build_plan("TEST", "hash123", &holdings, &[], 10_000.0, &prices);
        assert_eq!(plan.lines.len(), 3);
        assert!(plan.cash_remainder.abs() < 1e-6, "remainder was {}", plan.cash_remainder);
    }
}
