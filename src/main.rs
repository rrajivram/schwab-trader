mod accounts;
mod api;
mod auth;
mod blacklist;
mod config;
mod indices;
mod orders;
mod pricing;
mod rebalance;
mod registry;
mod stream;
mod tui;
mod watchlist;

use anyhow::Result;
use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(name = "schwab", about = "Schwab trading CLI / TUI")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Authenticate via OAuth and save credentials
    Login {
        #[arg(long, env = "SCHWAB_APP_KEY")]
        app_key: String,
        #[arg(long, env = "SCHWAB_APP_SECRET")]
        app_secret: String,
    },
    /// Get a one-shot quote for a symbol (no TUI)
    Quote {
        /// Stock symbol, e.g. AAPL
        symbol: String,
    },
    /// Launch the full TUI with streaming watchlists
    Tui,
    /// Show stored credentials and token status
    Status,
    /// Submit a single-leg BUY order. Prints the request body and exits
    /// without sending anything unless --live is passed.
    Order {
        /// Stock ticker (equity) or CUSIP (fixed-income), e.g. F or
        /// 912797FZ0.
        symbol: String,
        #[arg(long)]
        quantity: f64,
        /// EQUITY submits a MARKET order (the default). FIXED_INCOME
        /// (bonds/Treasuries) submits a LIMIT order and requires --price;
        /// this path has never been tested against Schwab's live endpoint.
        #[arg(long, value_enum, default_value = "equity")]
        asset_type: orders::AssetType,
        /// Limit price for --asset-type fixed-income. Ignored for equity
        /// orders (which are always MARKET).
        #[arg(long)]
        price: Option<f64>,
        /// Schwab account hash to trade in. Defaults to the single linked
        /// account, resolved live (account hashes can go stale, so the
        /// TUI-cached one in config is deliberately not used here).
        #[arg(long)]
        account_hash: Option<String>,
        /// Actually submit the order. Without this flag, only the request
        /// body that would be sent is printed.
        #[arg(long)]
        live: bool,
    },
    /// Print a Black-Scholes theoretical price grid (11 strikes x 11 IVs,
    /// each swept -50%..+50% around the live underlying price / base IV in
    /// 10% steps) for a European-style approximation of an option.
    Price {
        /// Underlying stock symbol, e.g. AAPL. Its current price is fetched
        /// live via the existing quotes API.
        #[arg(long)]
        symbol: String,
        /// Option expiration date, YYYY-MM-DD. Must be in the future.
        #[arg(long)]
        expiry: String,
        /// Base implied volatility as a decimal, e.g. 0.30 for 30%. Swept
        /// -50%..+50% across the grid's columns.
        #[arg(long)]
        iv: f64,
        #[arg(long, value_enum)]
        option_type: pricing::OptionType,
        /// Annualized risk-free rate. This codebase has no live rate
        /// source, so this is a literal default, overridable via this flag.
        #[arg(long, default_value_t = 0.045)]
        rate: f64,
        /// Annualized dividend yield. No live source; literal default,
        /// overridable via this flag.
        #[arg(long, default_value_t = 0.0)]
        dividend_yield: f64,
    },
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Commands::Login { app_key, app_secret } => {
            auth::login(&app_key, &app_secret).await?;
        }
        Commands::Quote { symbol } => {
            api::get_quote(&symbol).await?;
        }
        Commands::Tui => {
            tui::run().await?;
        }
        Commands::Status => {
            config::print_status()?;
        }
        Commands::Order { symbol, quantity, asset_type, price, account_hash, live } => {
            orders::place_order_cli(&symbol, quantity, asset_type, price, account_hash, live).await?;
        }
        Commands::Price { symbol, expiry, iv, option_type, rate, dividend_yield } => {
            pricing::run_price_cli(&symbol, &expiry, iv, option_type, rate, dividend_yield).await?;
        }
    }

    Ok(())
}
