//! Composite stock score for picking: value, quality, momentum and low beta,
//! each ranked against the stock's own sector so a utility's P/E is compared
//! with other utilities, not with software companies.
//!
//! Each input becomes a within-sector percentile (0 = worst, 100 = best).
//! A factor is the mean of its inputs' percentiles; the score is the
//! weighted mean of the factors a stock has. Percentiles rather than
//! z-scores keep outliers (ROE of 150%, P/E of 900) from dominating.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

use crate::api::MarketData;

/// Relative factor weights (any non-negative numbers; only ratios matter).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Weights {
    pub value: f64,
    pub quality: f64,
    pub momentum: f64,
    pub low_beta: f64,
}

impl Default for Weights {
    fn default() -> Self {
        Self { value: 25.0, quality: 25.0, momentum: 25.0, low_beta: 25.0 }
    }
}

impl Weights {
    fn as_array(&self) -> [f64; 4] {
        [self.value, self.quality, self.momentum, self.low_beta].map(|w| w.max(0.0))
    }
}

/// Raw inputs for one stock, oriented so higher is better except where noted.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Inputs {
    /// 1 / P/E. Losses give a negative yield, ranking below every profit.
    pub earnings_yield: Option<f64>,
    /// 1 / price-to-cash-flow.
    pub cash_flow_yield: Option<f64>,
    pub roe: Option<f64>,
    pub net_margin: Option<f64>,
    /// Total debt / equity — lower is better. Negative equity is +∞.
    pub leverage: Option<f64>,
    /// Price return from 12 months ago to 1 month ago.
    pub momentum: Option<f64>,
    /// Lower is better.
    pub beta: Option<f64>,
}

impl Inputs {
    /// `fit_beta` (our regression) is preferred over Schwab's published one.
    pub fn new(md: Option<&MarketData>, closes: Option<&[(i64, f64)]>, fit_beta: Option<f64>) -> Self {
        let md = md.cloned().unwrap_or_default();
        // A P/E in (0, 1) is bad data, not a bargain (BRK.B's quote divides
        // by Class A EPS), and Schwab sends 0 for "unknown".
        let pe = md.pe_ratio.filter(|&pe| pe <= 0.0 || pe >= 1.0).filter(|&pe| pe != 0.0);
        let pcf = md.pcf_ratio.filter(|&v| v != 0.0);
        // Negative equity: maximally levered, and ROE is meaningless.
        let negative_equity = md.debt_to_equity.is_some_and(|d| d < 0.0);
        Self {
            earnings_yield: pe.map(|pe| 1.0 / pe),
            cash_flow_yield: pcf.map(|v| 1.0 / v),
            roe: md.roe.filter(|&v| v != 0.0 && !negative_equity),
            net_margin: md.net_margin.filter(|&v| v != 0.0),
            // 0 is Schwab's "not applicable" (every big bank).
            leverage: md.debt_to_equity.filter(|&v| v != 0.0).map(|d| if d < 0.0 { f64::INFINITY } else { d }),
            momentum: closes.and_then(momentum_12_1),
            beta: fit_beta.or(md.beta),
        }
    }
}

/// Weekly closes → return from 52 weeks ago to 4 weeks ago. Skipping the
/// last month avoids its short-term reversal effect.
pub fn momentum_12_1(closes: &[(i64, f64)]) -> Option<f64> {
    let n = closes.len();
    if n < 53 {
        return None;
    }
    let (start, end) = (closes[n - 53].1, closes[n - 5].1);
    (start > 0.0).then(|| end / start - 1.0)
}

/// Factor percentiles (0–100, higher is better) and their weighted blend.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Score {
    pub total: f64,
    pub value: Option<f64>,
    pub quality: Option<f64>,
    pub momentum: Option<f64>,
    pub low_beta: Option<f64>,
}

impl Score {
    pub fn breakdown(&self) -> String {
        let f = |v: Option<f64>| v.map(|v| format!("{v:.0}")).unwrap_or("—".into());
        format!("Value {} · Quality {} · Momentum {} · Low beta {}", f(self.value), f(self.quality), f(self.momentum), f(self.low_beta))
    }
}

/// Percentile rank of each present value among the present ones, 0..100,
/// ties sharing their average rank. A lone value gets 50.
fn percentiles(values: &[Option<f64>]) -> Vec<Option<f64>> {
    let mut idx: Vec<usize> = (0..values.len()).filter(|&i| values[i].is_some()).collect();
    idx.sort_by(|&a, &b| values[a].unwrap().total_cmp(&values[b].unwrap()));
    let n = idx.len();
    let mut out = vec![None; values.len()];
    let mut i = 0;
    while i < n {
        let mut j = i;
        while j + 1 < n && values[idx[j + 1]] == values[idx[i]] {
            j += 1;
        }
        let rank = (i + j) as f64 / 2.0;
        let pct = if n > 1 { 100.0 * rank / (n - 1) as f64 } else { 50.0 };
        for &k in &idx[i..=j] {
            out[k] = Some(pct);
        }
        i = j + 1;
    }
    out
}

fn mean(xs: impl IntoIterator<Item = Option<f64>>) -> Option<f64> {
    let (sum, n) = xs.into_iter().flatten().fold((0.0, 0), |(s, n), x| (s + x, n + 1));
    (n > 0).then(|| sum / n as f64)
}

/// Score every stock against its sector. `stocks` is (symbol, sector,
/// inputs). A stock needs at least two weighted factors (or the only one,
/// if just one has weight) to get a score.
pub fn score(stocks: &[(String, String, Inputs)], weights: &Weights) -> HashMap<String, Score> {
    let w = weights.as_array();
    let needed = w.iter().filter(|&&x| x > 0.0).count().min(2);
    let mut by_sector: HashMap<&str, Vec<&(String, String, Inputs)>> = HashMap::new();
    for s in stocks {
        by_sector.entry(s.1.as_str()).or_default().push(s);
    }

    let mut out = HashMap::new();
    for group in by_sector.values() {
        let pct = |f: &dyn Fn(&Inputs) -> Option<f64>| percentiles(&group.iter().map(|s| f(&s.2)).collect::<Vec<_>>());
        let ey = pct(&|i| i.earnings_yield);
        let cfy = pct(&|i| i.cash_flow_yield);
        let roe = pct(&|i| i.roe);
        let margin = pct(&|i| i.net_margin);
        let lev = pct(&|i| i.leverage.map(|v| -v));
        let mom = pct(&|i| i.momentum);
        let beta = pct(&|i| i.beta.map(|v| -v));

        for (k, s) in group.iter().enumerate() {
            let factors = [mean([ey[k], cfy[k]]), mean([roe[k], margin[k], lev[k]]), mom[k], beta[k]];
            let (sum, weight, count) = factors.iter().zip(w).filter(|(f, w)| f.is_some() && *w > 0.0).fold(
                (0.0, 0.0, 0),
                |(s, ws, n), (f, w)| (s + f.unwrap() * w, ws + w, n + 1),
            );
            if count >= needed && weight > 0.0 {
                let [value, quality, momentum, low_beta] = factors;
                out.insert(s.0.clone(), Score { total: sum / weight, value, quality, momentum, low_beta });
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stock(sym: &str, sector: &str, i: Inputs) -> (String, String, Inputs) {
        (sym.into(), sector.into(), i)
    }

    #[test]
    fn percentiles_rank_and_share_ties() {
        let p = percentiles(&[Some(3.0), None, Some(1.0), Some(3.0), Some(2.0)]);
        let rounded: Vec<Option<f64>> = p.iter().map(|v| v.map(|x| (x * 100.0).round() / 100.0)).collect();
        assert_eq!(rounded, vec![Some(83.33), None, Some(0.0), Some(83.33), Some(33.33)]);
        assert_eq!(percentiles(&[Some(7.0)]), vec![Some(50.0)]);
    }

    #[test]
    fn ranks_within_sector_not_across() {
        // Same cheapness relative to sector peers → same value score, despite
        // very different absolute P/Es.
        let i = |pe: f64| Inputs { earnings_yield: Some(1.0 / pe), beta: Some(1.0), ..Default::default() };
        let s = score(
            &[stock("UTIL1", "Utilities", i(12.0)), stock("UTIL2", "Utilities", i(18.0)), stock("TECH1", "Tech", i(30.0)), stock("TECH2", "Tech", i(60.0))],
            &Weights::default(),
        );
        assert_eq!(s["UTIL1"].value, Some(100.0));
        assert_eq!(s["TECH1"].value, Some(100.0));
        assert_eq!(s["UTIL2"].value, Some(0.0));
    }

    #[test]
    fn lower_beta_and_leverage_rank_higher() {
        let i = |beta: f64, lev: f64| Inputs { beta: Some(beta), leverage: Some(lev), ..Default::default() };
        let s = score(&[stock("A", "X", i(0.5, 20.0)), stock("B", "X", i(1.5, f64::INFINITY))], &Weights::default());
        assert_eq!(s["A"].low_beta, Some(100.0));
        assert_eq!(s["A"].quality, Some(100.0));
        assert_eq!(s["B"].total, 0.0);
    }

    #[test]
    fn weights_blend_available_factors_only() {
        let a = Inputs { earnings_yield: Some(0.1), momentum: Some(-0.2), ..Default::default() };
        let b = Inputs { earnings_yield: Some(0.05), momentum: Some(0.3), ..Default::default() };
        let w = Weights { value: 3.0, quality: 0.0, momentum: 1.0, low_beta: 0.0 };
        let s = score(&[stock("A", "X", a), stock("B", "X", b)], &w);
        // A: value 100, momentum 0 → (300 + 0) / 4.
        assert_eq!(s["A"].total, 75.0);
        assert_eq!(s["B"].total, 25.0);
    }

    #[test]
    fn needs_two_factors() {
        let one = Inputs { earnings_yield: Some(0.1), ..Default::default() };
        let s = score(&[stock("A", "X", one.clone())], &Weights::default());
        assert!(s.is_empty());
        // Unless only one factor is weighted at all.
        let s = score(&[stock("A", "X", one)], &Weights { value: 1.0, quality: 0.0, momentum: 0.0, low_beta: 0.0 });
        assert_eq!(s["A"].total, 50.0);
    }

    #[test]
    fn bad_and_sentinel_fundamentals_are_unknown() {
        let md = MarketData {
            pe_ratio: Some(0.00845),
            pcf_ratio: Some(0.0),
            roe: Some(40.0),
            debt_to_equity: Some(-150.0),
            ..Default::default()
        };
        let i = Inputs::new(Some(&md), None, None);
        assert_eq!(i.earnings_yield, None);
        assert_eq!(i.cash_flow_yield, None);
        assert_eq!(i.roe, None, "ROE is meaningless with negative equity");
        assert_eq!(i.leverage, Some(f64::INFINITY));
        // Banks: D/E 0 means not applicable. Losses: negative earnings yield.
        let bank = MarketData { debt_to_equity: Some(0.0), pe_ratio: Some(-8.0), ..Default::default() };
        let i = Inputs::new(Some(&bank), None, Some(1.2));
        assert_eq!(i.leverage, None);
        assert_eq!(i.earnings_yield, Some(-0.125));
        assert_eq!(i.beta, Some(1.2));
    }

    #[test]
    fn momentum_skips_the_last_month() {
        let closes: Vec<(i64, f64)> = (0..60).map(|i| (i, 100.0 + i as f64)).collect();
        // 52 weeks back = index 7 (107), 4 weeks back = index 55 (155).
        assert!((momentum_12_1(&closes).unwrap() - (155.0 / 107.0 - 1.0)).abs() < 1e-12);
        assert_eq!(momentum_12_1(&closes[..52]), None);
    }
}
