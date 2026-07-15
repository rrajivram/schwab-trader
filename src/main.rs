mod accounts;
mod api;
mod auth;
mod blacklist;
mod config;
mod indices;
mod orders;
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
    /// Submit a single-leg market BUY order. Prints the request body and
    /// exits without sending anything unless --live is passed.
    Order {
        /// Stock symbol, e.g. F
        symbol: String,
        #[arg(long)]
        quantity: f64,
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
        Commands::Order { symbol, quantity, account_hash, live } => {
            orders::place_order_cli(&symbol, quantity, account_hash, live).await?;
        }
    }

    Ok(())
}
