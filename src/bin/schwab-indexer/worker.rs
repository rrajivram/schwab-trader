//! Background work: a tokio runtime owned by the GUI. Each request runs as a
//! task and reports back over a std channel, waking the UI with a repaint.

use std::sync::mpsc::{channel, Receiver, Sender};

use eframe::egui;

use std::collections::HashMap;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;

use schwab::{accounts, alphavantage, api, auth, dividends, execution, history, universe};

pub enum Msg {
    /// Startup token check: Ok means a usable (possibly refreshed) token exists.
    TokenChecked(Result<(), String>),
    LoginBegun(Result<String, String>),
    LoginCompleted(Result<(), String>),
    AccountLoaded(Result<accounts::Account, String>),
    UniverseLoaded(Result<universe::Universe, String>),
    MarketLoaded(Result<HashMap<String, api::MarketData>, String>),
    DividendsLoaded(Result<HashMap<String, f64>, String>),
    /// Streamed progress from an order run.
    Exec(execution::ExecEvent),
    /// One symbol's Alpha Vantage overview arrived (already cached to disk).
    Overview(String, alphavantage::Overview),
    /// Something worth telling the user about the Alpha Vantage run.
    OverviewNote(String),
    /// Run finished; carries requests used today.
    OverviewsDone(u32),
    /// Live 13-week T-bill yield, for fitting alpha.
    RiskFree(Result<f64, String>),
    /// One symbol's weekly closes arrived (already cached to disk).
    History(String, history::Series),
    /// Price-history run finished; carries the symbols that failed.
    HistoryDone(Vec<String>),
}

pub struct Worker {
    rt: tokio::runtime::Runtime,
    tx: Sender<Msg>,
    pub rx: Receiver<Msg>,
    ctx: egui::Context,
}

impl Worker {
    pub fn new(ctx: egui::Context) -> Self {
        let (tx, rx) = channel();
        let rt = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .expect("failed to start tokio runtime");
        Self { rt, tx, rx, ctx }
    }

    fn spawn<F>(&self, fut: F)
    where
        F: std::future::Future<Output = Msg> + Send + 'static,
    {
        let tx = self.tx.clone();
        let ctx = self.ctx.clone();
        self.rt.spawn(async move {
            let _ = tx.send(fut.await);
            ctx.request_repaint();
        });
    }

    pub fn check_token(&self) {
        self.spawn(async {
            Msg::TokenChecked(auth::get_valid_token().await.map(|_| ()).map_err(|e| e.to_string()))
        });
    }

    /// Not async, but kept on the worker so every result arrives the same way.
    pub fn begin_login(&self, app_key: String, app_secret: String) {
        self.spawn(async move {
            Msg::LoginBegun(auth::begin_login(&app_key, &app_secret).map_err(|e| e.to_string()))
        });
    }

    pub fn complete_login(&self, pasted_url: String) {
        self.spawn(async move {
            Msg::LoginCompleted(auth::complete_login(&pasted_url).await.map_err(|e| e.to_string()))
        });
    }

    /// The token only has access to a single account, so take the first one.
    /// The hash is re-fetched every time rather than cached (hashes go stale).
    pub fn load_account(&self) {
        self.spawn(async {
            let result = async {
                let hashes = accounts::list_account_numbers().await?;
                let first = hashes
                    .first()
                    .ok_or_else(|| anyhow::anyhow!("This token has no linked accounts"))?;
                accounts::get_account(first).await
            }
            .await;
            Msg::AccountLoaded(result.map_err(|e| e.to_string()))
        });
    }

    pub fn load_universe(&self, force: bool) {
        self.spawn(async move {
            Msg::UniverseLoaded(universe::load(force).await.map_err(|e| e.to_string()))
        });
    }

    pub fn load_market(&self, symbols: Vec<String>) {
        self.spawn(async move {
            let result = async {
                let token = auth::get_valid_token().await?;
                api::fetch_market_data(&token, &symbols).await
            }
            .await;
            Msg::MarketLoaded(result.map_err(|e| e.to_string()))
        });
    }

    pub fn load_dividends(&self, account_hash: String, held_symbols: Vec<String>) {
        self.spawn(async move {
            let result = dividends::dividends_by_symbol(&account_hash, &held_symbols).await;
            Msg::DividendsLoaded(result.map_err(|e| e.to_string()))
        });
    }

    pub fn run_orders(&self, plan: execution::Plan, cancel: Arc<AtomicBool>) {
        let tx = self.tx.clone();
        let ctx = self.ctx.clone();
        self.rt.spawn(async move {
            execution::run(plan, cancel, move |ev| {
                let _ = tx.send(Msg::Exec(ev));
                ctx.request_repaint();
            })
            .await;
        });
    }

    /// Fetch the risk-free rate, then weekly closes one symbol at a time
    /// (benchmark first) for symbols not freshly cached. Paced to stay under
    /// Schwab's ~120 requests a minute alongside the app's other calls.
    pub fn load_history(&self, symbols: Vec<String>, need_rate: bool) {
        let tx = self.tx.clone();
        let ctx = self.ctx.clone();
        self.rt.spawn(async move {
            let send = |m: Msg| {
                let _ = tx.send(m);
                ctx.request_repaint();
            };
            let token = match auth::get_valid_token().await {
                Ok(t) => t,
                Err(e) => {
                    if need_rate {
                        send(Msg::RiskFree(Err(e.to_string())));
                    }
                    return send(Msg::HistoryDone(symbols));
                }
            };
            if need_rate {
                send(Msg::RiskFree(api::fetch_risk_free_rate(&token).await.map_err(|e| e.to_string())));
            }
            let mut cache = history::Cache::load();
            let mut failed = Vec::new();
            for (i, sym) in symbols.into_iter().enumerate() {
                if i > 0 {
                    tokio::time::sleep(std::time::Duration::from_millis(550)).await;
                }
                match history::fetch_weekly(&token, &sym).await {
                    Ok(s) => {
                        cache.series.insert(sym.clone(), s.clone());
                        send(Msg::History(sym, s));
                    }
                    Err(_) => failed.push(sym),
                }
                if i % 25 == 24 {
                    let _ = cache.save();
                }
            }
            let _ = cache.save();
            send(Msg::HistoryDone(failed));
        });
    }

    /// Fetch Alpha Vantage overviews one at a time for symbols not freshly
    /// cached, stopping at the daily limit. Spaced out to avoid Alpha
    /// Vantage's burst limit.
    pub fn load_overviews(&self, api_key: String, symbols: Vec<String>) {
        let tx = self.tx.clone();
        let ctx = self.ctx.clone();
        self.rt.spawn(async move {
            let send = |m: Msg| {
                let _ = tx.send(m);
                ctx.request_repaint();
            };
            let mut cache = alphavantage::Cache::load();
            let mut first = true;
            for sym in symbols {
                if cache.overviews.get(&sym).is_some_and(|o| o.is_fresh()) {
                    continue;
                }
                if cache.remaining_today() == 0 {
                    send(Msg::OverviewNote(format!(
                        "Daily limit of {} Alpha Vantage requests reached. The rest load tomorrow.",
                        alphavantage::DAILY_LIMIT
                    )));
                    break;
                }
                if !first {
                    tokio::time::sleep(std::time::Duration::from_millis(1500)).await;
                }
                first = false;
                match alphavantage::fetch_overview(&api_key, &sym, &mut cache).await {
                    Ok(o) => {
                        cache.overviews.insert(sym.clone(), o.clone());
                        send(Msg::Overview(sym, o));
                    }
                    Err(alphavantage::FetchError::RateLimited(msg)) => {
                        let _ = cache.save();
                        send(Msg::OverviewNote(format!("Alpha Vantage limit reached: {msg}")));
                        break;
                    }
                    Err(alphavantage::FetchError::Other(e)) => {
                        send(Msg::OverviewNote(format!("{sym}: {e}")));
                    }
                }
                let _ = cache.save();
            }
            send(Msg::OverviewsDone(cache.used_today()));
        });
    }
}
