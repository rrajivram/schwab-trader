use serde::{Deserialize, Serialize};

/// Which ETF issuer's public daily holdings file backs a registry entry.
/// Each issuer publishes holdings in a different file format/URL shape.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum HoldingsProvider {
    /// State Street (SPDR) — daily xlsx, verified reachable with a plain GET.
    Ssga,
    /// BlackRock (iShares) — daily csv; a plain GET returned an HTML page
    /// despite CSV-looking response headers during testing, so this needs
    /// investigation before it can be trusted (see feed_url comment).
    IShares,
    /// Invesco — daily csv; a plain GET returned HTTP 406 during testing,
    /// consistent with bot-fingerprint blocking rather than a bad URL.
    Invesco,
    /// Vanguard — not used by any seeded default yet, kept for user-added
    /// entries (e.g. Vanguard index fund ETFs) once its feed shape is verified.
    Vanguard,
}

/// Maps an index name a user might type (e.g. "SIXB") to the ETF that tracks
/// it 1:1 and the public holdings feed to pull symbol+weight data from.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RegistryEntry {
    /// Canonical key, matched case-insensitively (e.g. "XLB").
    pub name: String,
    /// Other names/tickers that should resolve to this entry (e.g. "SIXB").
    pub aliases: Vec<String>,
    /// Human-readable display name.
    pub label: String,
    /// Ticker of the ETF whose holdings are used as the weight proxy.
    pub tracking_etf: String,
    pub provider: HoldingsProvider,
    /// URL of the issuer's public daily holdings file for `tracking_etf`.
    pub feed_url: String,
}
