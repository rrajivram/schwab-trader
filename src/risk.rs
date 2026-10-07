//! Risk measures for stock selection: beta (how much a stock moves with the
//! market) and, from price history, alpha (return beyond what beta explains).

/// Value-weighted beta of a set of positions.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct WeightedBeta {
    /// Weighted over just the positions with a known beta.
    pub beta: f64,
    /// Value of the positions with a known beta.
    pub covered: f64,
    /// Number of positions with a known beta.
    pub count: usize,
}

impl WeightedBeta {
    /// Beta of a larger total (e.g. the whole account) where everything
    /// outside `covered` — cash, bonds — counts as beta 0.
    pub fn diluted(&self, total: f64) -> Option<f64> {
        (total > 0.0).then(|| self.beta * self.covered / total)
    }
}

/// Value-weighted beta over `(value, beta)` pairs, skipping unknown betas
/// and non-positive values. None when nothing is left to weight.
pub fn weighted_beta(items: impl IntoIterator<Item = (f64, Option<f64>)>) -> Option<WeightedBeta> {
    let (mut sum, mut covered, mut count) = (0.0, 0.0, 0);
    for (value, beta) in items {
        if let (true, Some(b)) = (value > 0.0, beta) {
            sum += value * b;
            covered += value;
            count += 1;
        }
    }
    (covered > 0.0).then(|| WeightedBeta { beta: sum / covered, covered, count })
}

pub const WEEKS_PER_YEAR: f64 = 52.0;
/// Fewer aligned weekly returns than this and the fit is too noisy to show.
pub const MIN_WEEKS: usize = 52;

/// Regression of a stock's weekly excess returns on the market's.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Fit {
    /// Jensen's alpha, annualized, as a decimal (0.05 = 5%/yr beyond what
    /// beta explains).
    pub alpha: f64,
    /// t-statistic of alpha. |t| < 2 means alpha can't be told from noise.
    pub alpha_t: f64,
    pub beta: f64,
    /// Share of the stock's weekly variance the market explains (0..1).
    pub r2: f64,
    /// Weekly returns used.
    pub weeks: usize,
}

impl Fit {
    pub fn significant(&self) -> bool {
        self.alpha_t.abs() >= 2.0
    }
}

/// OLS fit of (stock − rf) on (market − rf) over weekly closes keyed by
/// week. Only weeks present in both series count, and a return is only
/// taken between consecutive market weeks, so gaps in the stock's history
/// (new listings, halts) don't become multi-week "weekly" returns.
/// `rf_annual` is today's rate applied to the whole window — a shortcut that
/// only touches alpha via rf × (1 − beta).
pub fn fit(stock: &[(i64, f64)], market: &[(i64, f64)], rf_annual: f64) -> Option<Fit> {
    let stock: std::collections::HashMap<i64, f64> = stock.iter().copied().collect();
    let rf = (1.0 + rf_annual).powf(1.0 / WEEKS_PER_YEAR) - 1.0;
    let (xs, ys): (Vec<f64>, Vec<f64>) = market
        .windows(2)
        .filter_map(|w| {
            let ((d0, m0), (d1, m1)) = (w[0], w[1]);
            let (s0, s1) = (*stock.get(&d0)?, *stock.get(&d1)?);
            (m0 > 0.0 && s0 > 0.0).then(|| (m1 / m0 - 1.0 - rf, s1 / s0 - 1.0 - rf))
        })
        .unzip();
    let n = xs.len();
    if n < MIN_WEEKS {
        return None;
    }
    let nf = n as f64;
    let (mx, my) = (xs.iter().sum::<f64>() / nf, ys.iter().sum::<f64>() / nf);
    let (mut sxx, mut sxy, mut syy) = (0.0, 0.0, 0.0);
    for (x, y) in xs.iter().zip(&ys) {
        sxx += (x - mx) * (x - mx);
        sxy += (x - mx) * (y - my);
        syy += (y - my) * (y - my);
    }
    if sxx <= 0.0 {
        return None;
    }
    let beta = sxy / sxx;
    let a = my - beta * mx;
    let sse = (syy - beta * sxy).max(0.0);
    let se_a = (sse / (nf - 2.0) * (1.0 / nf + mx * mx / sxx)).sqrt();
    Some(Fit {
        alpha: a * WEEKS_PER_YEAR,
        alpha_t: if se_a > 0.0 { a / se_a } else { f64::INFINITY.copysign(a) },
        beta,
        r2: if syy > 0.0 { 1.0 - sse / syy } else { 1.0 },
        weeks: n,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Weekly closes from returns, one week (ms) apart.
    fn closes(returns: &[f64]) -> Vec<(i64, f64)> {
        const WEEK: i64 = 7 * 24 * 3600 * 1000;
        let mut p = 100.0;
        let mut out = vec![(0, p)];
        for (i, r) in returns.iter().enumerate() {
            p *= 1.0 + r;
            out.push(((i as i64 + 1) * WEEK, p));
        }
        out
    }

    fn market_returns(n: usize) -> Vec<f64> {
        (0..n).map(|i| 0.02 * ((i as f64) * 1.7).sin() + 0.001).collect()
    }

    #[test]
    fn recovers_exact_alpha_and_beta() {
        let rf = 0.04;
        let rfw = (1.0f64 + rf).powf(1.0 / 52.0) - 1.0;
        let m = market_returns(156);
        // Stock excess = 0.001/wk + 1.5 × market excess.
        let s: Vec<f64> = m.iter().map(|r| rfw + 0.001 + 1.5 * (r - rfw)).collect();
        let f = fit(&closes(&s), &closes(&m), rf).unwrap();
        assert!((f.beta - 1.5).abs() < 1e-9, "{f:?}");
        assert!((f.alpha - 0.052).abs() < 1e-9, "{f:?}");
        assert!((f.r2 - 1.0).abs() < 1e-9);
        assert_eq!(f.weeks, 156);
    }

    #[test]
    fn noise_lowers_r2_and_t() {
        let m = market_returns(156);
        // Deterministic "noise" uncorrelated with the market's sine.
        let s: Vec<f64> = m.iter().enumerate().map(|(i, r)| 0.8 * r + 0.03 * ((i as f64) * 0.37).cos()).collect();
        let f = fit(&closes(&s), &closes(&m), 0.0).unwrap();
        assert!((f.beta - 0.8).abs() < 0.15, "{f:?}");
        assert!(f.r2 < 0.6, "{f:?}");
        assert!(f.alpha_t.is_finite());
    }

    #[test]
    fn gaps_are_skipped_not_bridged() {
        let m = market_returns(100);
        let mut s = closes(&m);
        // Drop a stock week: the two returns touching it go, nothing spans it.
        s.remove(50);
        let f = fit(&s, &closes(&m), 0.0).unwrap();
        assert_eq!(f.weeks, 98);
        assert!((f.beta - 1.0).abs() < 1e-9);
    }

    #[test]
    fn too_short_is_none() {
        let m = market_returns(MIN_WEEKS - 1);
        assert!(fit(&closes(&m), &closes(&m), 0.0).is_none());
    }

    #[test]
    fn weights_by_value_and_skips_unknowns() {
        // $3000 at 2.0 and $1000 at 0.4: (6000 + 400) / 4000 = 1.6.
        let w = weighted_beta([(3000.0, Some(2.0)), (1000.0, Some(0.4)), (5000.0, None), (0.0, Some(9.0))]).unwrap();
        assert!((w.beta - 1.6).abs() < 1e-12);
        assert_eq!(w.covered, 4000.0);
        assert_eq!(w.count, 2);
    }

    #[test]
    fn nothing_known_is_none() {
        assert!(weighted_beta([(100.0, None)]).is_none());
        assert!(weighted_beta(std::iter::empty()).is_none());
    }

    #[test]
    fn diluted_treats_the_rest_as_zero_beta() {
        let w = weighted_beta([(4000.0, Some(1.5))]).unwrap();
        assert_eq!(w.diluted(10_000.0), Some(0.6));
        assert_eq!(w.diluted(0.0), None);
    }
}
