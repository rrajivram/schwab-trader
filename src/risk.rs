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

#[cfg(test)]
mod tests {
    use super::*;

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
