//! The S&P 500 universe for schwab-indexer: each constituent's index weight
//! (from SPY's daily holdings) and GICS sector (from which of the 11 Select
//! Sector SPDRs holds it — together they partition the S&P 500 exactly).

use anyhow::{Context, Result};
use std::collections::HashMap;
use std::path::PathBuf;

use crate::registry::{self, holdings::Holding};

/// Select Sector SPDR ticker → GICS sector name.
pub const SECTOR_ETFS: &[(&str, &str)] = &[
    ("XLB", "Materials"),
    ("XLC", "Communication Services"),
    ("XLE", "Energy"),
    ("XLF", "Financials"),
    ("XLI", "Industrials"),
    ("XLK", "Information Technology"),
    ("XLP", "Consumer Staples"),
    ("XLRE", "Real Estate"),
    ("XLU", "Utilities"),
    ("XLV", "Health Care"),
    ("XLY", "Consumer Discretionary"),
];

/// Companies listed under more than one share class, merged into one line:
/// (secondary class → primary class). The primary is the class kept.
pub const SHARE_CLASSES: &[(&str, &str)] = &[
    ("GOOG", "GOOGL"),
    ("FOX", "FOXA"),
    ("NWS", "NWSA"),
];

/// The ticker a symbol is listed under in the universe: Schwab's `BRK/B`
/// becomes the index's `BRK.B`, and secondary share classes map to their
/// primary (GOOG → GOOGL).
pub fn universe_symbol(symbol: &str) -> String {
    let s = symbol.replace('/', ".");
    SHARE_CLASSES
        .iter()
        .find(|(secondary, _)| *secondary == s)
        .map(|(_, primary)| primary.to_string())
        .unwrap_or(s)
}

#[derive(Debug, Clone)]
pub struct Constituent {
    pub symbol: String,
    pub sector: String,
    /// Fraction of the index (0.07 = 7%), including any merged share classes.
    pub weight: f64,
    /// Other share-class tickers folded into this line (e.g. GOOG under GOOGL).
    pub merged: Vec<String>,
}

#[derive(Debug, Clone, Default)]
pub struct Universe {
    pub constituents: Vec<Constituent>,
    /// SPY feed's own "as of" date, e.g. "As of 24-Sep-2026".
    pub source_as_of: Option<String>,
    /// Non-fatal problems worth showing the user (stale cache, dropped symbols).
    pub warnings: Vec<String>,
}

impl Universe {
    /// Sectors ordered by total index weight, heaviest first, with that weight.
    pub fn sectors_by_weight(&self) -> Vec<(String, f64)> {
        let mut totals: HashMap<&str, f64> = HashMap::new();
        for c in &self.constituents {
            *totals.entry(c.sector.as_str()).or_default() += c.weight;
        }
        let mut v: Vec<(String, f64)> = totals.into_iter().map(|(s, w)| (s.to_string(), w)).collect();
        v.sort_by(|a, b| b.1.total_cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
        v
    }
}

/// Fetch SPY + all sector SPDR holdings (cached for 24h unless `force`) and
/// build the universe. Sector ETFs that can't be fetched fall back to
/// `~/Downloads/sp500.csv` for sector lookups.
pub async fn load(force: bool) -> Result<Universe> {
    let entries = registry::load()?;
    let spy_entry = registry::find(&entries, "SPY").context("SPY missing from index registry")?;
    let spy = registry::get_holdings_with(spy_entry, force).await?;

    let mut warnings = Vec::new();
    if let Some(e) = &spy.fetch_error {
        warnings.push(format!("Using cached SPY weights — refresh failed: {e}"));
    }

    let mut sector_holdings = Vec::new();
    for (etf, sector) in SECTOR_ETFS {
        let result = match registry::find(&entries, etf) {
            Some(entry) => registry::get_holdings_with(entry, force).await,
            None => Err(anyhow::anyhow!("{etf} missing from index registry")),
        };
        match result {
            Ok(h) => {
                if let Some(e) = &h.fetch_error {
                    warnings.push(format!("Using cached {etf} holdings — refresh failed: {e}"));
                }
                sector_holdings.push((sector.to_string(), h.constituents));
            }
            Err(e) => warnings.push(format!("{etf} ({sector}) unavailable, using sp500.csv: {e}")),
        }
    }

    let fallback = load_fallback_sectors().unwrap_or_default();
    let mut universe = build(&spy.constituents, &sector_holdings, &fallback);
    universe.source_as_of = spy.source_as_of;
    warnings.append(&mut universe.warnings);
    universe.warnings = warnings;
    Ok(universe)
}

/// Pure assembly step: attach sectors, merge share classes, drop anything
/// with no known sector, and drop placeholder lines like TPG's "2602335D"
/// (non-tradable rights/CVR codes, which some sector SPDRs also list).
pub fn build(
    spy: &[Holding],
    sector_holdings: &[(String, Vec<Holding>)],
    fallback_sectors: &HashMap<String, String>,
) -> Universe {
    let mut sector_of: HashMap<&str, &str> = HashMap::new();
    for (sector, holdings) in sector_holdings {
        for h in holdings {
            sector_of.insert(h.symbol.as_str(), sector.as_str());
        }
    }

    let mut by_symbol: HashMap<String, Constituent> = HashMap::new();
    let mut dropped = Vec::new();

    for h in spy {
        if h.symbol.starts_with(|c: char| c.is_ascii_digit()) {
            dropped.push(h.symbol.clone());
            continue;
        }
        let sector = sector_of
            .get(h.symbol.as_str())
            .copied()
            .or_else(|| fallback_sectors.get(&h.symbol).map(String::as_str));
        let Some(sector) = sector else {
            dropped.push(h.symbol.clone());
            continue;
        };

        let primary = SHARE_CLASSES
            .iter()
            .find(|(secondary, _)| *secondary == h.symbol)
            .map(|(_, primary)| primary.to_string());
        let key = primary.clone().unwrap_or_else(|| h.symbol.clone());

        let c = by_symbol.entry(key.clone()).or_insert_with(|| Constituent {
            symbol: key,
            sector: sector.to_string(),
            weight: 0.0,
            merged: Vec::new(),
        });
        c.weight += h.weight;
        if primary.is_some() {
            c.merged.push(h.symbol.clone());
        }
    }

    let mut constituents: Vec<Constituent> = by_symbol.into_values().collect();
    constituents.sort_by(|a, b| b.weight.total_cmp(&a.weight).then_with(|| a.symbol.cmp(&b.symbol)));

    let mut warnings = Vec::new();
    if !dropped.is_empty() {
        warnings.push(format!("Not a tradable ticker or no sector found, excluded: {}", dropped.join(", ")));
    }

    Universe { constituents, source_as_of: None, warnings }
}

fn fallback_csv_path() -> Option<PathBuf> {
    Some(dirs::home_dir()?.join("Downloads").join("sp500.csv"))
}

/// `~/Downloads/sp500.csv` (Symbol, ..., GICS Sector, ...) → symbol → sector.
fn load_fallback_sectors() -> Result<HashMap<String, String>> {
    let path = fallback_csv_path().context("no home directory")?;
    let mut rdr = csv::Reader::from_path(&path)?;
    let headers = rdr.headers()?.clone();
    let sym_col = headers.iter().position(|h| h == "Symbol").context("no Symbol column")?;
    let sec_col = headers.iter().position(|h| h == "GICS Sector").context("no GICS Sector column")?;

    let mut map = HashMap::new();
    for rec in rdr.records() {
        let rec = rec?;
        if let (Some(sym), Some(sec)) = (rec.get(sym_col), rec.get(sec_col)) {
            map.insert(sym.trim().to_uppercase(), sec.trim().to_string());
        }
    }
    Ok(map)
}

/// Search match on a stock's ticker or name: case-insensitive, every
/// whitespace-separated term must appear in one of them, and ticker
/// punctuation is ignored (so `brk.b`, `BRK-B` and `brk/b` all find BRK.B).
/// `also` takes extra tickers that count as this stock's (merged classes).
pub fn matches_query(query: &str, symbol: &str, name: &str, also: &[String]) -> bool {
    let squash = |s: &str| s.chars().filter(|c| c.is_alphanumeric()).collect::<String>().to_lowercase();
    let tickers: Vec<String> = std::iter::once(symbol).chain(also.iter().map(String::as_str)).map(squash).collect();
    let name = name.to_lowercase();
    query.split_whitespace().all(|term| {
        let t = term.to_lowercase();
        let tq = squash(term);
        name.contains(&t) || (!tq.is_empty() && tickers.iter().any(|k| k.contains(&tq)))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn search_matches_ticker_or_name() {
        let m = |q: &str| matches_query(q, "BRK.B", "Berkshire Hathaway Inc", &[]);
        assert!(m(""), "empty query matches everything");
        assert!(m("brk"));
        assert!(m("BRK-B") && m("brk/b") && m("brk.b"));
        assert!(m("berkshire"));
        assert!(m("hath inc"), "every term, any field");
        assert!(!m("berkshire apple"));
        assert!(!m("msft"));
        // Merged share classes count as the stock's tickers.
        assert!(matches_query("goog", "GOOGL", "Alphabet Inc", &["GOOG".into()]));
        assert!(!matches_query(".", "A", "Agilent", &[]), "punctuation alone isn't a ticker search");
    }

    fn h(symbol: &str, weight: f64) -> Holding {
        Holding { symbol: symbol.into(), weight }
    }

    #[test]
    fn merges_share_classes_into_primary() {
        let spy = [h("GOOGL", 0.02), h("GOOG", 0.015), h("AAPL", 0.07)];
        let sectors = [
            ("Communication Services".to_string(), vec![h("GOOGL", 0.2), h("GOOG", 0.1)]),
            ("Information Technology".to_string(), vec![h("AAPL", 0.2)]),
        ];
        let u = build(&spy, &sectors, &HashMap::new());
        assert_eq!(u.constituents.len(), 2);
        let googl = u.constituents.iter().find(|c| c.symbol == "GOOGL").unwrap();
        assert!((googl.weight - 0.035).abs() < 1e-12);
        assert_eq!(googl.merged, vec!["GOOG".to_string()]);
    }

    #[test]
    fn secondary_class_alone_is_listed_under_primary_ticker() {
        let spy = [h("FOX", 0.001)];
        let sectors = [("Communication Services".to_string(), vec![h("FOX", 0.01)])];
        let u = build(&spy, &sectors, &HashMap::new());
        assert_eq!(u.constituents[0].symbol, "FOXA");
    }

    #[test]
    fn falls_back_to_csv_sector_then_drops_unknowns() {
        let spy = [h("AAPL", 0.07), h("XOM", 0.01), h("2602335D", 0.00001)];
        let sectors = [
            ("Information Technology".to_string(), vec![h("AAPL", 0.2)]),
            ("Health Care".to_string(), vec![h("2602335D", 0.00001)]),
        ];
        let fallback = HashMap::from([("XOM".to_string(), "Energy".to_string())]);
        let u = build(&spy, &sectors, &fallback);
        let syms: Vec<&str> = u.constituents.iter().map(|c| c.symbol.as_str()).collect();
        assert_eq!(syms, vec!["AAPL", "XOM"]);
        assert_eq!(u.constituents[1].sector, "Energy");
        assert!(u.warnings[0].contains("2602335D"));
    }

    #[test]
    fn sectors_ordered_by_total_weight() {
        let spy = [h("A", 0.03), h("B", 0.03), h("C", 0.05)];
        let sectors = [
            ("S1".to_string(), vec![h("A", 0.5), h("B", 0.5)]),
            ("S2".to_string(), vec![h("C", 1.0)]),
        ];
        let u = build(&spy, &sectors, &HashMap::new());
        let order: Vec<String> = u.sectors_by_weight().into_iter().map(|(s, _)| s).collect();
        assert_eq!(order, vec!["S1", "S2"]);
    }

    /// Hits SSGA for SPY + 11 sector SPDRs. Run with `cargo test -- --ignored`.
    #[tokio::test]
    #[ignore]
    async fn live_universe_covers_the_index() {
        let u = load(true).await.unwrap();
        assert!(u.constituents.len() > 490, "got {}", u.constituents.len());
        assert_eq!(u.sectors_by_weight().len(), 11);
        let total: f64 = u.constituents.iter().map(|c| c.weight).sum();
        assert!((0.97..=1.02).contains(&total), "total weight {total}");
        println!("{} constituents, total {total:.4}, warnings {:?}", u.constituents.len(), u.warnings);
    }
}
