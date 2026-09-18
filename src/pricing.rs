use anyhow::{bail, Context, Result};
use chrono::NaiveDate;
use clap::ValueEnum;

/// Call or put.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum OptionType {
    Call,
    Put,
}

/// Applied to the live spot price and the user's base IV to build the grid's
/// two sweeps: -50% .. +50% in 10% increments. Index 5 is the unmultiplied
/// (spot / base-IV) point.
const SWEEP_MULTIPLIERS: [f64; 11] = [0.5, 0.6, 0.7, 0.8, 0.9, 1.0, 1.1, 1.2, 1.3, 1.4, 1.5];

/// An 11x11 Black-Scholes theoretical price grid: strikes (rows) x IVs
/// (columns), for one symbol/expiry/option-type at a fixed underlying price.
#[derive(Debug, Clone)]
pub struct PriceGrid {
    pub symbol: String,
    pub spot: f64,
    pub base_iv: f64,
    pub expiry: NaiveDate,
    pub time_to_expiry_years: f64,
    pub rate: f64,
    pub dividend_yield: f64,
    pub option_type: OptionType,
    /// Ascending, len 11. `strikes[5] == spot`.
    pub strikes: Vec<f64>,
    /// Ascending, len 11. `ivs[5] == base_iv`.
    pub ivs: Vec<f64>,
    /// `prices[row][col]` = price at `(strikes[row], ivs[col])`.
    pub prices: Vec<Vec<f64>>,
}

/// Abramowitz & Stegun 7.1.26 rational approximation (max error ~1.5e-7).
fn erf(x: f64) -> f64 {
    let sign = if x < 0.0 { -1.0 } else { 1.0 };
    let x = x.abs();

    let a1 = 0.254829592;
    let a2 = -0.284496736;
    let a3 = 1.421413741;
    let a4 = -1.453152027;
    let a5 = 1.061405429;
    let p = 0.3275911;

    let t = 1.0 / (1.0 + p * x);
    let y = 1.0 - (((((a5 * t + a4) * t) + a3) * t + a2) * t + a1) * t * (-x * x).exp();
    sign * y
}

pub fn normal_cdf(x: f64) -> f64 {
    0.5 * (1.0 + erf(x / std::f64::consts::SQRT_2))
}

/// European Black-Scholes-Merton price with a continuous dividend yield.
/// Falls back to intrinsic value when `time_to_expiry_years` or `iv` is
/// non-positive, to avoid a division by zero at degenerate sweep inputs.
///
/// This is a European approximation applied to American-style equity
/// options — it ignores early-exercise value, which is deliberately out of
/// scope (would require a binomial/trinomial tree).
pub fn black_scholes_price(
    spot: f64,
    strike: f64,
    time_to_expiry_years: f64,
    rate: f64,
    dividend_yield: f64,
    iv: f64,
    option_type: OptionType,
) -> f64 {
    if time_to_expiry_years <= 0.0 || iv <= 0.0 {
        return match option_type {
            OptionType::Call => (spot - strike).max(0.0),
            OptionType::Put => (strike - spot).max(0.0),
        };
    }

    let sqrt_t = time_to_expiry_years.sqrt();
    let d1 = ((spot / strike).ln()
        + (rate - dividend_yield + 0.5 * iv * iv) * time_to_expiry_years)
        / (iv * sqrt_t);
    let d2 = d1 - iv * sqrt_t;

    let discounted_spot = spot * (-dividend_yield * time_to_expiry_years).exp();
    let discounted_strike = strike * (-rate * time_to_expiry_years).exp();

    match option_type {
        OptionType::Call => discounted_spot * normal_cdf(d1) - discounted_strike * normal_cdf(d2),
        OptionType::Put => discounted_strike * normal_cdf(-d2) - discounted_spot * normal_cdf(-d1),
    }
}

/// Pure sweep generators — no I/O.
pub fn strike_sweep(spot: f64) -> Vec<f64> {
    SWEEP_MULTIPLIERS.iter().map(|m| spot * m).collect()
}

pub fn iv_sweep(base_iv: f64) -> Vec<f64> {
    SWEEP_MULTIPLIERS.iter().map(|m| base_iv * m).collect()
}

/// Year-fraction from calendar days, using a 365.25-day year — a standard
/// equity-option convention, consistent with the already-approximate
/// European Black-Scholes model being applied here.
pub fn year_fraction(today: NaiveDate, expiry: NaiveDate) -> f64 {
    (expiry - today).num_days() as f64 / 365.25
}

/// Pure grid builder — no I/O, fully unit-testable.
#[allow(clippy::too_many_arguments)]
pub fn build_price_grid(
    symbol: &str,
    spot: f64,
    base_iv: f64,
    expiry: NaiveDate,
    today: NaiveDate,
    rate: f64,
    dividend_yield: f64,
    option_type: OptionType,
) -> Result<PriceGrid> {
    if expiry <= today {
        bail!("expiry {expiry} must be after today ({today})");
    }
    if spot <= 0.0 {
        bail!("underlying price must be positive, got {spot}");
    }
    if base_iv <= 0.0 {
        bail!("base IV must be positive, got {base_iv}");
    }

    let time_to_expiry_years = year_fraction(today, expiry);
    let strikes = strike_sweep(spot);
    let ivs = iv_sweep(base_iv);
    let prices = strikes
        .iter()
        .map(|&k| {
            ivs.iter()
                .map(|&v| {
                    black_scholes_price(spot, k, time_to_expiry_years, rate, dividend_yield, v, option_type)
                })
                .collect()
        })
        .collect();

    Ok(PriceGrid {
        symbol: symbol.to_string(),
        spot,
        base_iv,
        expiry,
        time_to_expiry_years,
        rate,
        dividend_yield,
        option_type,
        strikes,
        ivs,
        prices,
    })
}

/// Fetches the live underlying price, then builds the pure grid.
pub async fn build_price_grid_live(
    symbol: &str,
    base_iv: f64,
    expiry: NaiveDate,
    rate: f64,
    dividend_yield: f64,
    option_type: OptionType,
) -> Result<PriceGrid> {
    let symbol_upper = symbol.to_uppercase();
    let token = crate::auth::get_valid_token().await?;
    let prices = crate::api::fetch_last_prices(&token, std::slice::from_ref(&symbol_upper)).await?;
    let spot = *prices
        .get(&symbol_upper)
        .with_context(|| format!("no live price available for {symbol_upper}"))?;
    let today = chrono::Utc::now().date_naive();
    build_price_grid(&symbol_upper, spot, base_iv, expiry, today, rate, dividend_yield, option_type)
}

/// CLI entry point for `schwab price`: parse/validate `--expiry`, fetch the
/// live grid, print it.
pub async fn run_price_cli(
    symbol: &str,
    expiry: &str,
    iv: f64,
    option_type: OptionType,
    rate: f64,
    dividend_yield: f64,
) -> Result<()> {
    let expiry = NaiveDate::parse_from_str(expiry, "%Y-%m-%d")
        .with_context(|| format!("invalid --expiry '{expiry}', expected YYYY-MM-DD"))?;
    let grid = build_price_grid_live(symbol, iv, expiry, rate, dividend_yield, option_type).await?;
    print_price_grid(&grid);
    Ok(())
}

/// Plain, manually aligned ASCII table (matches `api::print_quote`'s style).
pub fn print_price_grid(grid: &PriceGrid) {
    let option_label = match grid.option_type {
        OptionType::Call => "CALL",
        OptionType::Put => "PUT",
    };
    println!("Symbol:     {}", grid.symbol);
    println!("Spot:       ${:.2}", grid.spot);
    println!("Expiry:     {} ({:.3}y)", grid.expiry, grid.time_to_expiry_years);
    println!("Base IV:    {:.1}%", grid.base_iv * 100.0);
    println!("Rate / Div: {:.2}% / {:.2}%", grid.rate * 100.0, grid.dividend_yield * 100.0);
    println!("Type:       {}", option_label);
    println!();

    print!("{:>10}", "Strike");
    for &iv in &grid.ivs {
        print!("{:>9}", format!("{:.1}%", iv * 100.0));
    }
    println!();

    for (row, &strike) in grid.strikes.iter().enumerate() {
        print!("{:>10}", format!("${:.2}", strike));
        for &price in &grid.prices[row] {
            print!("{:>9}", format!("${:.2}", price));
        }
        println!();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normal_cdf_matches_known_values() {
        assert!((normal_cdf(0.0) - 0.5).abs() < 1e-6);
        assert!((normal_cdf(1.0) - 0.8413447).abs() < 1e-6);
        assert!((normal_cdf(-1.0) - 0.1586553).abs() < 1e-6);
    }

    #[test]
    fn black_scholes_matches_textbook_example() {
        // S=100, K=100, r=5%, q=0%, sigma=20%, T=1y: call ~= 10.4506, put ~= 5.5735
        let call = black_scholes_price(100.0, 100.0, 1.0, 0.05, 0.0, 0.20, OptionType::Call);
        let put = black_scholes_price(100.0, 100.0, 1.0, 0.05, 0.0, 0.20, OptionType::Put);
        assert!((call - 10.4506).abs() < 1e-3, "call was {call}");
        assert!((put - 5.5735).abs() < 1e-3, "put was {put}");
    }

    fn sample_grid(option_type: OptionType) -> PriceGrid {
        let today = NaiveDate::from_ymd_opt(2026, 1, 1).unwrap();
        let expiry = NaiveDate::from_ymd_opt(2027, 1, 1).unwrap();
        build_price_grid("AAPL", 100.0, 0.20, expiry, today, 0.05, 0.0, option_type).unwrap()
    }

    #[test]
    fn grid_atm_cell_matches_direct_bs_call() {
        let grid = sample_grid(OptionType::Call);
        assert!((grid.strikes[5] - 100.0).abs() < 1e-9);
        assert!((grid.ivs[5] - 0.20).abs() < 1e-9);
        let direct = black_scholes_price(
            grid.spot,
            grid.strikes[5],
            grid.time_to_expiry_years,
            grid.rate,
            grid.dividend_yield,
            grid.ivs[5],
            OptionType::Call,
        );
        assert!((grid.prices[5][5] - direct).abs() < 1e-9);
    }

    #[test]
    fn call_price_decreases_as_strike_rises() {
        let grid = sample_grid(OptionType::Call);
        for i in 0..grid.strikes.len() - 1 {
            assert!(grid.prices[i][5] >= grid.prices[i + 1][5]);
        }
    }

    #[test]
    fn put_price_increases_as_strike_rises() {
        let grid = sample_grid(OptionType::Put);
        for i in 0..grid.strikes.len() - 1 {
            assert!(grid.prices[i][5] <= grid.prices[i + 1][5]);
        }
    }

    #[test]
    fn price_increases_monotonically_with_iv() {
        for option_type in [OptionType::Call, OptionType::Put] {
            let grid = sample_grid(option_type);
            for j in 0..grid.ivs.len() - 1 {
                assert!(grid.prices[5][j] <= grid.prices[5][j + 1]);
            }
        }
    }

    #[test]
    fn rejects_expiry_not_in_the_future() {
        let today = NaiveDate::from_ymd_opt(2026, 1, 1).unwrap();
        let result = build_price_grid("AAPL", 100.0, 0.20, today, today, 0.05, 0.0, OptionType::Call);
        assert!(result.is_err());
    }
}
