mod app;
mod events;
mod ui;

use std::io;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::Result;
use crossterm::{
    event::{DisableMouseCapture, EnableMouseCapture, EventStream},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use futures_util::StreamExt;
use ratatui::{backend::CrosstermBackend, Terminal};
use tokio::sync::mpsc;

use crate::{accounts, api, auth, blacklist, orders, rebalance, registry, stream, watchlist};
use app::{App, AccountsResult, LookupResult, PlanBuildResult};

pub async fn run() -> Result<()> {
    let _ = auth::get_valid_token().await?; // fail fast if not logged in

    // WebSocket streamer channels
    let (quote_tx, mut quote_rx) = mpsc::channel::<stream::QuoteUpdate>(2000);
    let (cmd_tx, cmd_rx)         = mpsc::channel::<stream::StreamCommand>(200);

    // REST enrichment channel — receives a list of symbols, fetches fundamentals
    let (enrich_tx, mut enrich_rx) = mpsc::channel::<Vec<String>>(50);
    let enrich_quote_tx = quote_tx.clone();
    tokio::spawn(async move {
        while let Some(symbols) = enrich_rx.recv().await {
            let token = match auth::get_valid_token().await {
                Ok(t) => t,
                Err(_) => continue,
            };
            match api::fetch_bulk_quotes(&token, &symbols).await {
                Ok(updates) => {
                    for u in updates {
                        let _ = enrich_quote_tx.try_send(u);
                    }
                }
                Err(_) => {}
            }
        }
    });

    // Index lookup channel — receives a query string, resolves it against the
    // index registry, fetches/parses holdings, and sends the result back.
    let (lookup_tx, mut lookup_rx) = mpsc::channel::<String>(20);
    let (lookup_result_tx, mut lookup_result_rx) = mpsc::channel::<LookupResult>(20);
    tokio::spawn(async move {
        while let Some(query) = lookup_rx.recv().await {
            let result = match registry::load() {
                Ok(entries) => match registry::find(&entries, &query) {
                    Some(entry) => match registry::get_holdings(entry).await {
                        Ok(holdings) => LookupResult::Found { entry: entry.clone(), holdings },
                        Err(e) => LookupResult::Error(e.to_string()),
                    },
                    None => LookupResult::NotFound(query.clone()),
                },
                Err(e) => LookupResult::Error(e.to_string()),
            };
            let _ = lookup_result_tx.try_send(result);
        }
    });

    // Account select channel — receives a fetch request, lists linked
    // accounts + positions, and sends the result back.
    let (accounts_tx, mut accounts_rx) = mpsc::channel::<()>(5);
    let (accounts_result_tx, mut accounts_result_rx) = mpsc::channel::<AccountsResult>(5);
    tokio::spawn(async move {
        while accounts_rx.recv().await.is_some() {
            let result = match accounts::list_accounts().await {
                Ok(accts) => AccountsResult::Loaded(accts),
                Err(e) => AccountsResult::Error(e.to_string()),
            };
            let _ = accounts_result_tx.try_send(result);
        }
    });

    // Plan-build channel — receives a PlanRequest, fetches prices, builds the
    // RebalancePlan, and sends the result back.
    let (plan_tx, mut plan_rx) = mpsc::channel::<app::PlanRequest>(5);
    let (plan_result_tx, mut plan_result_rx) = mpsc::channel::<PlanBuildResult>(5);
    tokio::spawn(async move {
        while let Some(req) = plan_rx.recv().await {
            let result = match rebalance::plan_investment(
                &req.index_name,
                &req.account_hash,
                &req.holdings,
                &req.blacklist,
                req.total_amount,
            )
            .await
            {
                Ok(plan) => PlanBuildResult::Ready(plan),
                Err(e) => PlanBuildResult::Error(e.to_string()),
            };
            let _ = plan_result_tx.try_send(result);
        }
    });

    // Order submission channel — receives a SubmitRequest, submits every
    // line in the plan (dry-run only from every current call site), and
    // sends the per-line results back.
    let (submit_tx, mut submit_rx) = mpsc::channel::<app::SubmitRequest>(5);
    let (submit_result_tx, mut submit_result_rx) = mpsc::channel::<Vec<orders::OrderResult>>(5);
    tokio::spawn(async move {
        while let Some(req) = submit_rx.recv().await {
            let results = orders::submit_plan(&req.plan, req.dry_run).await;
            let _ = submit_result_tx.try_send(results);
        }
    });

    // Shared WebSocket status string shown in the status bar
    let ws_status = Arc::new(Mutex::new("Connecting...".to_string()));

    // Spawn persistent streamer task
    let status_clone = Arc::clone(&ws_status);
    tokio::spawn(stream::run(quote_tx, cmd_rx, status_clone));

    // Load persisted watchlists and blacklist
    let watchlists = watchlist::load().unwrap_or_default();
    let blacklist = blacklist::load().unwrap_or_default();

    // Terminal setup
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    // Build initial app state
    let mut app = App::new(
        watchlists,
        Arc::clone(&ws_status),
        cmd_tx.clone(),
        enrich_tx,
        blacklist,
        lookup_tx,
        accounts_tx,
        plan_tx,
        submit_tx,
    );

    // Subscribe to the first tab's symbols right away
    app.push_symbols();

    // Event loop
    let mut event_stream = EventStream::new();
    let mut tick = tokio::time::interval(Duration::from_millis(100));

    loop {
        tokio::select! {
            _ = tick.tick() => {}

            maybe_ev = event_stream.next() => {
                use crossterm::event::Event;
                if let Some(Ok(Event::Key(key))) = maybe_ev {
                    events::handle_key(key, &mut app);
                }
            }

            Some(update) = quote_rx.recv() => {
                app.apply_update(update);
            }

            Some(result) = lookup_result_rx.recv() => {
                app.apply_lookup_result(result);
            }

            Some(result) = accounts_result_rx.recv() => {
                app.apply_accounts_result(result);
            }

            Some(result) = plan_result_rx.recv() => {
                app.apply_plan_result(result);
            }

            Some(results) = submit_result_rx.recv() => {
                app.apply_submit_result(results);
            }
        }

        terminal.draw(|f| ui::render(f, &mut app))?;

        if app.should_quit {
            break;
        }
    }

    // Signal streamer to stop
    let _ = cmd_tx.send(stream::StreamCommand::Quit).await;

    // Restore terminal
    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen, DisableMouseCapture)?;
    terminal.show_cursor()?;

    Ok(())
}
