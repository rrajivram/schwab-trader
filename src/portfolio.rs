//! The account's long positions keyed the way the indexer universe is
//! (share classes merged), with dividends folded into the gain/loss.

use std::collections::HashMap;

use crate::accounts::Account;
use crate::universe::universe_symbol;

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Held {
    pub quantity: f64,
    /// Total cost basis (quantity × average cost).
    pub cost: f64,
    pub market_value: f64,
    /// Dividends received on this symbol (best-effort history window).
    pub dividends: f64,
}

impl Held {
    /// Unrealized gain/loss including dividends received.
    pub fn total_return(&self) -> f64 {
        self.market_value + self.dividends - self.cost
    }

    /// Below purchase price once dividends are counted.
    pub fn is_losing(&self) -> bool {
        self.total_return() < 0.0
    }
}

/// Long equity positions only (e.g. Treasuries are skipped).
pub fn held_positions(account: &Account, dividends: &HashMap<String, f64>) -> HashMap<String, Held> {
    let mut out: HashMap<String, Held> = HashMap::new();
    for p in account.positions.iter().filter(|p| p.long_quantity > 0.0 && p.asset_type == "EQUITY") {
        let h = out.entry(universe_symbol(&p.symbol)).or_default();
        h.quantity += p.long_quantity;
        h.cost += p.long_quantity * p.cost_basis_per_share;
        h.market_value += p.market_value;
    }
    for (symbol, amount) in dividends {
        if let Some(h) = out.get_mut(&universe_symbol(symbol)) {
            h.dividends += amount;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::accounts::Position;

    fn pos(symbol: &str, qty: f64, cost_per_share: f64, market_value: f64) -> Position {
        Position {
            symbol: symbol.into(),
            asset_type: "EQUITY".into(),
            description: None,
            long_quantity: qty,
            short_quantity: 0.0,
            cost_basis_per_share: cost_per_share,
            market_value,
            long_open_profit_loss: market_value - qty * cost_per_share,
        }
    }

    fn account(positions: Vec<Position>) -> Account {
        Account {
            account_number: "1234".into(),
            hash_value: "h".into(),
            account_type: "CASH".into(),
            cash_balance: 0.0,
            liquidation_value: 0.0,
            positions,
        }
    }

    #[test]
    fn dividends_can_turn_a_price_loss_into_a_gain() {
        let acct = account(vec![pos("KO", 10.0, 60.0, 590.0)]);
        let divs = HashMap::from([("KO".to_string(), 20.0)]);
        let held = held_positions(&acct, &divs);
        assert!((held["KO"].total_return() - 10.0).abs() < 1e-9);
        assert!(!held["KO"].is_losing());
    }

    #[test]
    fn share_classes_are_merged_under_primary() {
        let acct = account(vec![pos("GOOG", 1.0, 100.0, 90.0), pos("GOOGL", 2.0, 100.0, 180.0)]);
        let held = held_positions(&acct, &HashMap::new());
        assert_eq!(held.len(), 1);
        assert_eq!(held["GOOGL"].quantity, 3.0);
        assert!(held["GOOGL"].is_losing());
    }
}
