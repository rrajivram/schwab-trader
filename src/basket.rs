//! Basket selection for schwab-indexer's Create and Rebalance modes.
//!
//! Auto logic: drop excluded stocks (do-not-transact, already held, ...),
//! rescale the remaining index weights back to 100%, then walk the sectors
//! heaviest-first taking each sector's highest-scoring stock, and repeat
//! (2nd highest, 3rd, ...) until the basket is full. Sectors that run out
//! are skipped. The score is whatever the caller ranks by (the indexer uses
//! its factor score); stocks with no score are still eligible, ranked last.

use std::collections::{HashMap, HashSet};

use crate::universe::Universe;

#[derive(Debug, Clone, PartialEq)]
pub struct Pick {
    pub symbol: String,
    pub sector: String,
    /// Index weight rescaled over the non-excluded universe (fraction of 1).
    pub weight: f64,
    /// The ranking metric (factor score in the indexer); None when unknown.
    pub score: Option<f64>,
}

/// Non-excluded candidates grouped by sector, sectors in index-weight order,
/// each sector's list ranked best-first (score, then weight, then symbol).
fn ranked_by_sector(universe: &Universe, scores: &HashMap<String, f64>, exclude: &HashSet<String>) -> Vec<(String, Vec<Pick>)> {
    let remaining: f64 = universe
        .constituents
        .iter()
        .filter(|c| !exclude.contains(&c.symbol))
        .map(|c| c.weight)
        .sum();

    universe
        .sectors_by_weight()
        .into_iter()
        .map(|(sector, _)| {
            let mut picks: Vec<Pick> = universe
                .constituents
                .iter()
                .filter(|c| c.sector == sector && !exclude.contains(&c.symbol))
                .map(|c| Pick {
                    symbol: c.symbol.clone(),
                    sector: c.sector.clone(),
                    weight: if remaining > 0.0 { c.weight / remaining } else { 0.0 },
                    score: scores.get(&c.symbol).copied(),
                })
                .collect();
            picks.sort_by(|a, b| {
                // Unknown scores rank below every real one, negatives included.
                let key = |p: &Pick| p.score.unwrap_or(f64::NEG_INFINITY);
                key(b)
                    .total_cmp(&key(a))
                    .then_with(|| b.weight.total_cmp(&a.weight))
                    .then_with(|| a.symbol.cmp(&b.symbol))
            });
            (sector, picks)
        })
        .collect()
}

/// Auto mode: up to `size` stocks, round-robin across sectors.
pub fn auto_basket(universe: &Universe, scores: &HashMap<String, f64>, exclude: &HashSet<String>, size: usize) -> Vec<Pick> {
    let sectors = ranked_by_sector(universe, scores, exclude);
    let mut out = Vec::with_capacity(size);
    for round in 0.. {
        let mut took_any = false;
        for (_, picks) in &sectors {
            if out.len() == size {
                return out;
            }
            if let Some(p) = picks.get(round) {
                out.push(p.clone());
                took_any = true;
            }
        }
        if !took_any {
            break; // every sector exhausted
        }
    }
    out
}

/// Rebalance mode: one replacement per discarded stock. A discard from
/// sector A is replaced by the best remaining stock in the next sector
/// after A (index-weight order, wrapping around, never A itself); if that
/// sector is exhausted, the one after it, and so on. `discard_sectors` is
/// the sector of each discarded stock, in order; `exclude` must already
/// contain the discards themselves. A discard with no replacement available
/// anywhere is skipped.
pub fn replacements(
    universe: &Universe,
    scores: &HashMap<String, f64>,
    exclude: &HashSet<String>,
    discard_sectors: &[String],
) -> Vec<Pick> {
    let mut sectors = ranked_by_sector(universe, scores, exclude);
    let mut next_index = vec![0usize; sectors.len()];
    let mut out = Vec::new();

    for from in discard_sectors {
        let Some(a) = sectors.iter().position(|(s, _)| s == from) else { continue };
        let n = sectors.len();
        for step in 1..n {
            let i = (a + step) % n;
            let picks = &mut sectors[i].1;
            if let Some(p) = picks.get(next_index[i]) {
                out.push(p.clone());
                next_index[i] += 1;
                break;
            }
        }
    }
    out
}

/// A basket's weights rescaled to sum to 1 (equal weights if all are zero).
pub fn normalized(weights: &[f64]) -> Vec<f64> {
    let total: f64 = weights.iter().sum();
    if total > 0.0 {
        weights.iter().map(|w| w / total).collect()
    } else if weights.is_empty() {
        Vec::new()
    } else {
        vec![1.0 / weights.len() as f64; weights.len()]
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Allocation {
    pub dollars: f64,
    /// Fractional shares, floored to 4 decimals so the total never exceeds
    /// the investment amount.
    pub quantity: f64,
}

/// Split `amount` by `weights` (normalized here) and convert to shares.
/// A line with no usable price gets dollars but zero shares.
pub fn allocate(amount: f64, weights: &[f64], prices: &[Option<f64>]) -> Vec<Allocation> {
    normalized(weights)
        .iter()
        .zip(prices)
        .map(|(w, price)| {
            let dollars = amount * w;
            let quantity = match price {
                Some(p) if *p > 0.0 => (dollars / p * 10_000.0).floor() / 10_000.0,
                _ => 0.0,
            };
            Allocation { dollars, quantity }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::universe::Constituent;

    fn universe(rows: &[(&str, &str, f64)]) -> Universe {
        Universe {
            constituents: rows
                .iter()
                .map(|(s, sec, w)| Constituent { symbol: s.to_string(), sector: sec.to_string(), weight: *w, merged: vec![] })
                .collect(),
            ..Default::default()
        }
    }

    fn scores(rows: &[(&str, f64)]) -> HashMap<String, f64> {
        rows.iter().map(|(s, y)| (s.to_string(), *y)).collect()
    }

    fn syms(p: &[Pick]) -> Vec<&str> {
        p.iter().map(|p| p.symbol.as_str()).collect()
    }

    /// Tech (0.6) > Energy (0.3) > Utilities (0.1).
    fn sample() -> (Universe, HashMap<String, f64>) {
        let u = universe(&[
            ("T1", "Tech", 0.3),
            ("T2", "Tech", 0.2),
            ("T3", "Tech", 0.1),
            ("E1", "Energy", 0.2),
            ("E2", "Energy", 0.1),
            ("U1", "Utilities", 0.1),
        ]);
        // T3 has no score at all.
        let y = scores(&[("T1", 0.5), ("T2", 1.5), ("E1", 3.0), ("E2", 4.0), ("U1", 3.5)]);
        (u, y)
    }

    #[test]
    fn auto_round_robins_by_sector_weight_then_score() {
        let (u, y) = sample();
        let b = auto_basket(&u, &y, &HashSet::new(), 5);
        assert_eq!(syms(&b), vec!["T2", "E2", "U1", "T1", "E1"]);
    }

    #[test]
    fn auto_skips_exhausted_sectors_and_includes_unscored() {
        let (u, y) = sample();
        let b = auto_basket(&u, &y, &HashSet::new(), 10);
        assert_eq!(syms(&b), vec!["T2", "E2", "U1", "T1", "E1", "T3"]);
    }

    #[test]
    fn negative_score_ranks_above_unknown() {
        let u = universe(&[("A", "S", 0.5), ("B", "S", 0.4), ("C", "S", 0.1)]);
        let y = scores(&[("A", -2.0), ("C", 1.0)]);
        let b = auto_basket(&u, &y, &HashSet::new(), 3);
        assert_eq!(syms(&b), vec!["C", "A", "B"]);
    }

    #[test]
    fn auto_excludes_and_rescales_weights() {
        let (u, y) = sample();
        let exclude = HashSet::from(["T2".to_string(), "E2".to_string()]);
        let b = auto_basket(&u, &y, &exclude, 3);
        assert_eq!(syms(&b), vec!["T1", "E1", "U1"]);
        // Remaining index weight is 0.7, so T1's 0.3 becomes 0.3/0.7.
        assert!((b[0].weight - 0.3 / 0.7).abs() < 1e-12);
    }

    #[test]
    fn replacement_comes_from_next_sector_and_wraps() {
        let (u, y) = sample();
        // Discarding T2 (Tech) and U1 (Utilities, last sector → wraps to Tech).
        let exclude = HashSet::from(["T2".to_string(), "U1".to_string()]);
        let r = replacements(&u, &y, &exclude, &["Tech".into(), "Utilities".into()]);
        assert_eq!(syms(&r), vec!["E2", "T1"]);
    }

    #[test]
    fn replacement_skips_exhausted_next_sector() {
        let (u, y) = sample();
        // Energy has nothing left, so a Tech discard falls through to Utilities.
        let exclude = HashSet::from(["T1".to_string(), "E1".to_string(), "E2".to_string()]);
        let r = replacements(&u, &y, &exclude, &["Tech".into()]);
        assert_eq!(syms(&r), vec!["U1"]);
    }

    #[test]
    fn two_discards_from_same_sector_take_successive_picks() {
        let (u, y) = sample();
        let exclude = HashSet::from(["T1".to_string(), "T2".to_string()]);
        let r = replacements(&u, &y, &exclude, &["Tech".into(), "Tech".into()]);
        assert_eq!(syms(&r), vec!["E2", "E1"]);
    }

    #[test]
    fn allocate_splits_by_weight_and_floors_fractional_shares() {
        let a = allocate(1000.0, &[3.0, 1.0], &[Some(33.0), None]);
        assert!((a[0].dollars - 750.0).abs() < 1e-9);
        assert_eq!(a[0].quantity, 22.7272);
        assert_eq!(a[1].quantity, 0.0);
    }
}
