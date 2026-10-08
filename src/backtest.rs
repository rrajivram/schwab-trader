//! Price-only backtest of the indexer's picking rule: at each rebalance,
//! score every stock on momentum and low beta using only prices up to that
//! week, take the top `size` round-robin across sectors (as Auto fill does),
//! hold them equal-weighted until the next rebalance.
//!
//! Value and quality aren't tested: Schwab only has today's fundamentals,
//! and scoring the past with them would leak the future. The universe is
//! today's S&P 500, so failures and removals are missing (survivorship
//! bias) — which is why the strategy is also compared with an equal-weight
//! basket of the same survivors, rebalanced on the same schedule: the gap
//! between those two is what the picking itself added.

use std::collections::{HashMap, HashSet};

use crate::basket;
use crate::factors::{self, Inputs, Weights};
use crate::risk::{self, Fit, WEEKS_PER_YEAR};
use crate::universe::Universe;

/// Weekly closes before a rebalance needed to score a stock: 52 for
/// 12-month momentum (and a year of returns for beta).
const LOOKBACK: usize = 52;
/// Beta at each rebalance uses up to 3 years of trailing weeks.
const BETA_WINDOW: usize = 157;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Params {
    pub size: usize,
    /// Weeks between rebalances (4 ≈ monthly, 13 quarterly, 52 yearly).
    pub every_weeks: usize,
    pub momentum_weight: f64,
    pub low_beta_weight: f64,
    pub rf_annual: f64,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Stats {
    pub total_return: f64,
    pub cagr: f64,
    /// Annualized volatility of weekly returns.
    pub volatility: f64,
    pub sharpe: f64,
    /// Worst peak-to-trough fall, as a positive fraction.
    pub max_drawdown: f64,
    /// Against the benchmark; None for the benchmark itself.
    pub fit: Option<Fit>,
    /// Share of the portfolio traded per year (rebalanced baskets only).
    pub turnover: Option<f64>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Rebalance {
    pub date: i64,
    pub picks: Vec<String>,
    /// Share of the portfolio replaced at this rebalance (1.0 = all of it).
    pub turnover: f64,
}

#[derive(Debug, Clone)]
pub struct Curve {
    /// Growth of 1.0, one value per week in `Outcome::dates`.
    pub values: Vec<f64>,
    pub stats: Stats,
}

#[derive(Debug, Clone)]
pub struct Outcome {
    /// Week starts (epoch ms), first rebalance onward.
    pub dates: Vec<i64>,
    pub strategy: Curve,
    pub equal_weight: Curve,
    pub benchmark: Curve,
    pub rebalances: Vec<Rebalance>,
    /// Stocks with any price history (the rest of the universe can't be tested).
    pub universe_size: usize,
}

/// Prices on the benchmark's weekly calendar, forward-filled across gaps
/// once a stock has started trading (a halt isn't a zero).
struct Aligned {
    prices: HashMap<String, Vec<Option<f64>>>,
}

impl Aligned {
    fn new(calendar: &[i64], series: &HashMap<String, Vec<(i64, f64)>>) -> Self {
        let prices = series
            .iter()
            .map(|(sym, closes)| {
                let by_date: HashMap<i64, f64> = closes.iter().copied().collect();
                let mut last = None;
                let row = calendar
                    .iter()
                    .map(|d| {
                        if let Some(&p) = by_date.get(d).filter(|p| **p > 0.0) {
                            last = Some(p);
                        }
                        last
                    })
                    .collect();
                (sym.clone(), row)
            })
            .collect();
        Self { prices }
    }

    fn at(&self, sym: &str, t: usize) -> Option<f64> {
        self.prices.get(sym)?.get(t).copied().flatten()
    }
}

/// Momentum and beta per stock as of week `t`, from prices up to `t` only.
fn inputs_at(aligned: &Aligned, calendar: &[i64], market: &[f64], t: usize, rf: f64) -> HashMap<String, Inputs> {
    let from = (t + 1).saturating_sub(BETA_WINDOW);
    let market_window: Vec<(i64, f64)> = (from..=t).map(|i| (calendar[i], market[i])).collect();
    aligned
        .prices
        .iter()
        .filter_map(|(sym, row)| {
            row[t]?;
            let momentum = match (row[t - LOOKBACK], row[t - 4]) {
                (Some(a), Some(b)) => Some(b / a - 1.0),
                _ => None,
            };
            let closes: Vec<(i64, f64)> = (from..=t).filter_map(|i| Some((calendar[i], row[i]?))).collect();
            let beta = risk::fit(&closes, &market_window, rf).map(|f| f.beta);
            Some((sym.clone(), Inputs { momentum, beta, ..Default::default() }))
        })
        .collect()
}

/// Equal-weight buy-and-hold of `picks` from week `t`, rebalanced by the
/// caller. Tracks units so weights drift with prices between rebalances.
struct Book {
    value: f64,
    units: HashMap<String, f64>,
    turnovers: Vec<f64>,
}

impl Book {
    fn new() -> Self {
        Self { value: 1.0, units: HashMap::new(), turnovers: Vec::new() }
    }

    fn mark(&mut self, aligned: &Aligned, t: usize) -> f64 {
        if !self.units.is_empty() {
            self.value = self.units.iter().map(|(s, u)| u * aligned.at(s, t).unwrap_or(0.0)).sum();
        }
        self.value
    }

    /// Returns the share of the portfolio traded.
    fn rebalance(&mut self, aligned: &Aligned, t: usize, picks: &[String]) -> f64 {
        let value = self.mark(aligned, t);
        let old: HashMap<&str, f64> =
            self.units.iter().map(|(s, u)| (s.as_str(), u * aligned.at(s, t).unwrap_or(0.0) / value)).collect();
        let w = 1.0 / picks.len() as f64;
        let names: HashSet<&str> = old.keys().copied().chain(picks.iter().map(String::as_str)).collect();
        let new: HashSet<&str> = picks.iter().map(String::as_str).collect();
        let turnover = if self.units.is_empty() {
            1.0
        } else {
            0.5 * names.iter().map(|s| ((if new.contains(s) { w } else { 0.0 }) - old.get(s).unwrap_or(&0.0)).abs()).sum::<f64>()
        };
        self.units = picks.iter().filter_map(|s| Some((s.clone(), value * w / aligned.at(s, t)?))).collect();
        turnover
    }
}

pub fn run(universe: &Universe, series: &HashMap<String, Vec<(i64, f64)>>, benchmark: &[(i64, f64)], p: &Params) -> Option<Outcome> {
    let calendar: Vec<i64> = benchmark.iter().map(|&(d, _)| d).collect();
    let market: Vec<f64> = benchmark.iter().map(|&(_, c)| c).collect();
    let n = calendar.len();
    let start = LOOKBACK;
    if n < start + 2 || p.size == 0 || p.every_weeks == 0 {
        return None;
    }
    let in_index: HashMap<&str, &str> = universe.constituents.iter().map(|c| (c.symbol.as_str(), c.sector.as_str())).collect();
    let series: HashMap<String, Vec<(i64, f64)>> =
        series.iter().filter(|(s, _)| in_index.contains_key(s.as_str())).map(|(s, c)| (s.clone(), c.clone())).collect();
    let aligned = Aligned::new(&calendar, &series);
    let weights = Weights { value: 0.0, quality: 0.0, momentum: p.momentum_weight, low_beta: p.low_beta_weight };

    let (mut strat, mut ew) = (Book::new(), Book::new());
    let (mut strat_curve, mut ew_curve) = (Vec::new(), Vec::new());
    let mut rebalances = Vec::new();
    for t in start..n {
        if (t - start).is_multiple_of(p.every_weeks) && t + 1 < n {
            let inputs = inputs_at(&aligned, &calendar, &market, t, p.rf_annual);
            let stocks: Vec<(String, String, Inputs)> =
                inputs.iter().map(|(s, i)| (s.clone(), in_index[s.as_str()].to_string(), i.clone())).collect();
            let scores: HashMap<String, f64> = factors::score(&stocks, &weights).into_iter().map(|(s, sc)| (s, sc.total)).collect();
            // Only scored stocks with a price this week are buyable.
            let exclude: HashSet<String> = universe.constituents.iter().map(|c| c.symbol.clone()).filter(|s| !scores.contains_key(s)).collect();
            let picks: Vec<String> = basket::auto_basket(universe, &scores, &exclude, p.size).into_iter().map(|p| p.symbol).collect();
            if !picks.is_empty() {
                let turnover = strat.rebalance(&aligned, t, &picks);
                rebalances.push(Rebalance { date: calendar[t], picks, turnover });
            }
            let mut everyone: Vec<String> = inputs.into_keys().collect();
            everyone.sort();
            ew.rebalance(&aligned, t, &everyone);
        }
        strat_curve.push(strat.mark(&aligned, t));
        ew_curve.push(ew.mark(&aligned, t));
    }
    strat.turnovers = rebalances.iter().map(|r| r.turnover).collect();

    let dates = calendar[start..].to_vec();
    let bench_curve: Vec<f64> = market[start..].iter().map(|m| m / market[start]).collect();
    let per_year = WEEKS_PER_YEAR / p.every_weeks as f64;
    // The first rebalance is the initial purchase, not turnover.
    let turnover = (strat.turnovers.len() > 1).then(|| strat.turnovers[1..].iter().sum::<f64>() / (strat.turnovers.len() - 1) as f64 * per_year);
    let bench_closes: Vec<(i64, f64)> = dates.iter().copied().zip(bench_curve.iter().copied()).collect();
    let curve = |values: Vec<f64>, turnover: Option<f64>, vs_bench: bool| {
        let fit = if vs_bench { risk::fit(&dates.iter().copied().zip(values.iter().copied()).collect::<Vec<_>>(), &bench_closes, p.rf_annual) } else { None };
        Curve { stats: stats(&values, p.rf_annual, fit, turnover), values }
    };
    Some(Outcome {
        strategy: curve(strat_curve, turnover, true),
        equal_weight: curve(ew_curve, None, true),
        benchmark: curve(bench_curve, None, false),
        dates,
        rebalances,
        universe_size: aligned.prices.values().filter(|r| r.iter().any(Option::is_some)).count(),
    })
}

/// Summary statistics of a weekly growth curve.
pub fn stats(values: &[f64], rf_annual: f64, fit: Option<Fit>, turnover: Option<f64>) -> Stats {
    let returns: Vec<f64> = values.windows(2).map(|w| w[1] / w[0] - 1.0).collect();
    let weeks = returns.len().max(1) as f64;
    let total_return = values.last().unwrap_or(&1.0) / values.first().unwrap_or(&1.0) - 1.0;
    let mean = returns.iter().sum::<f64>() / weeks;
    let var = returns.iter().map(|r| (r - mean).powi(2)).sum::<f64>() / (weeks - 1.0).max(1.0);
    let volatility = var.sqrt() * WEEKS_PER_YEAR.sqrt();
    let rf_weekly = (1.0 + rf_annual).powf(1.0 / WEEKS_PER_YEAR) - 1.0;
    let (mut peak, mut max_drawdown) = (f64::MIN, 0.0f64);
    for &v in values {
        peak = peak.max(v);
        max_drawdown = max_drawdown.max(1.0 - v / peak);
    }
    Stats {
        total_return,
        cagr: (1.0 + total_return).powf(WEEKS_PER_YEAR / weeks) - 1.0,
        volatility,
        sharpe: if volatility > 0.0 { (mean - rf_weekly) * WEEKS_PER_YEAR / volatility } else { 0.0 },
        max_drawdown,
        fit,
        turnover,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::universe::Constituent;

    const WEEK: i64 = 7 * 24 * 3600 * 1000;

    fn universe(stocks: &[(&str, &str)]) -> Universe {
        Universe {
            constituents: stocks
                .iter()
                .map(|(s, sec)| Constituent { symbol: s.to_string(), sector: sec.to_string(), weight: 0.01, merged: Vec::new() })
                .collect(),
            source_as_of: None,
            warnings: Vec::new(),
        }
    }

    /// Closes growing by a weekly rate, with a market-like wiggle so beta fits.
    fn series(weeks: usize, rate: f64, wiggle: f64) -> Vec<(i64, f64)> {
        let mut p = 100.0;
        (0..weeks)
            .map(|i| {
                if i > 0 {
                    p *= 1.0 + rate + wiggle * 0.02 * ((i as f64) * 1.3).sin();
                }
                (i as i64 * WEEK, p)
            })
            .collect()
    }

    fn params(size: usize) -> Params {
        Params { size, every_weeks: 13, momentum_weight: 1.0, low_beta_weight: 0.0, rf_annual: 0.0 }
    }

    #[test]
    fn momentum_picks_the_winner_in_each_sector() {
        let u = universe(&[("UP", "A"), ("FLAT", "A"), ("UP2", "B"), ("DOWN", "B")]);
        let s: HashMap<String, Vec<(i64, f64)>> = [
            ("UP", series(120, 0.01, 1.0)),
            ("FLAT", series(120, 0.0, 1.0)),
            ("UP2", series(120, 0.005, 1.0)),
            ("DOWN", series(120, -0.005, 1.0)),
        ]
        .into_iter()
        .map(|(k, v)| (k.to_string(), v))
        .collect();
        let bench = series(120, 0.002, 1.0);
        let out = run(&u, &s, &bench, &params(2)).unwrap();
        assert!(out.rebalances.iter().all(|r| { let mut p = r.picks.clone(); p.sort(); p == ["UP", "UP2"] }), "{:?}", out.rebalances);
        // Same picks every time: only the first rebalance trades.
        assert_eq!(out.rebalances[0].turnover, 1.0);
        assert!(out.rebalances[1..].iter().all(|r| r.turnover < 0.2));
        assert!(out.strategy.stats.total_return > out.equal_weight.stats.total_return);
        assert_eq!(out.dates.len(), 120 - LOOKBACK);
        assert_eq!(out.strategy.values.len(), out.dates.len());
    }

    #[test]
    fn stocks_listing_late_are_skipped_until_they_have_history() {
        let u = universe(&[("OLD", "A"), ("NEW", "A")]);
        let late: Vec<(i64, f64)> = series(120, 0.03, 1.0).into_iter().skip(80).collect();
        let s: HashMap<String, Vec<(i64, f64)>> =
            [("OLD".to_string(), series(120, 0.0, 1.0)), ("NEW".to_string(), late)].into_iter().collect();
        let out = run(&u, &s, &series(120, 0.0, 1.0), &params(1)).unwrap();
        assert!(out.rebalances.iter().all(|r| r.picks == ["OLD"]), "{:?}", out.rebalances);
    }

    #[test]
    fn stats_on_a_known_curve() {
        // Up 10%, down to 0.88 (−20% from peak), up to 1.1.
        let s = stats(&[1.0, 1.1, 0.88, 1.1], 0.0, None, None);
        assert!((s.total_return - 0.1).abs() < 1e-12);
        assert!((s.max_drawdown - 0.2).abs() < 1e-12);
        assert!(s.volatility > 0.0);
    }

    #[test]
    fn too_little_history_is_none() {
        let u = universe(&[("A", "X")]);
        let s: HashMap<String, Vec<(i64, f64)>> = [("A".to_string(), series(40, 0.0, 1.0))].into_iter().collect();
        assert!(run(&u, &s, &series(40, 0.0, 1.0), &params(1)).is_none());
    }
}
