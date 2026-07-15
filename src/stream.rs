use anyhow::{bail, Result};
use futures_util::{SinkExt, StreamExt};
use serde::Deserialize;
use serde_json::{json, Value};
use std::collections::HashSet;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::mpsc;
use tokio_tungstenite::{connect_async, tungstenite::Message};

use crate::auth;

const USER_PREF_URL: &str = "https://api.schwabapi.com/trader/v1/userPreference";
// Only fields confirmed reliable from streaming: bid,ask,last,volume,day high,day low
const EQUITY_FIELDS: &str = "0,1,2,3,8,12,13";

// ── Public types ──────────────────────────────────────────────────────────────

#[derive(Debug, Default)]
pub struct QuoteUpdate {
    pub symbol: String,
    pub bid: Option<f64>,
    pub ask: Option<f64>,
    pub last: Option<f64>,
    pub volume: Option<u64>,
    pub high: Option<f64>,
    pub low: Option<f64>,
    pub prev_close: Option<f64>,
    pub description: Option<String>,
    pub open: Option<f64>,
    pub net_change: Option<f64>,
    pub w52_high: Option<f64>,
    pub w52_low: Option<f64>,
    pub pe_ratio: Option<f64>,
    pub div_amount: Option<f64>,
    pub div_yield: Option<f64>,
    pub eps: Option<f64>,
    pub market_cap: Option<f64>,
}

pub enum StreamCommand {
    SetSymbols(Vec<String>),
    Quit,
}

// ── Streamer info from /trader/v1/userPreference ──────────────────────────────

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct UserPrefResponse {
    streamer_info: Vec<StreamerInfoRaw>,
}

#[derive(Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
struct StreamerInfoRaw {
    streamer_socket_url: String,
    schwab_client_customer_id: String,
    schwab_client_correl_id: String,
    schwab_client_channel: String,
    schwab_client_function_id: String,
}

async fn fetch_streamer_info(token: &str) -> Result<StreamerInfoRaw> {
    let resp = reqwest::Client::new()
        .get(USER_PREF_URL)
        .header("Authorization", format!("Bearer {}", token))
        .send()
        .await?;

    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        if status.as_u16() == 401 {
            bail!(
                "401 Unauthorized on userPreference (body: {body})\n\n\
                Two common causes:\n\
                1. Your Schwab developer app is missing the \"Accounts and Trading Production\"\n\
                   API product. Go to developer.schwab.com -> your app -> Add Product.\n\
                2. Your token is stale. Run: schwab login --app-key ... --app-secret ..."
            );
        }
        bail!("userPreference failed ({status}): {body}");
    }

    let pref: UserPrefResponse = resp.json().await?;
    pref.streamer_info.into_iter().next().ok_or_else(|| anyhow::anyhow!("No streamer info in userPreference response"))
}

// ── Public entry point ────────────────────────────────────────────────────────

pub async fn run(
    quote_tx: mpsc::Sender<QuoteUpdate>,
    mut cmd_rx: mpsc::Receiver<StreamCommand>,
    status: Arc<Mutex<String>>,
) {
    let mut current: HashSet<String> = HashSet::new();
    let mut backoff = 2u64;

    loop {
        // Drain any pending commands before reconnecting.
        loop {
            match cmd_rx.try_recv() {
                Ok(StreamCommand::SetSymbols(syms)) => {
                    current = syms.into_iter().collect();
                }
                Ok(StreamCommand::Quit) | Err(mpsc::error::TryRecvError::Disconnected) => return,
                Err(mpsc::error::TryRecvError::Empty) => break,
            }
        }

        set_status(&status, "Connecting...");

        let token = match auth::get_valid_token().await {
            Ok(t) => t,
            Err(e) => {
                set_status(&status, &format!("Auth error: {e}"));
                sleep_with_drain(&mut cmd_rx, &mut current, backoff).await;
                backoff = (backoff * 2).min(60);
                continue;
            }
        };

        let info = match fetch_streamer_info(&token).await {
            Ok(i) => i,
            Err(e) => {
                set_status(&status, &format!("Streamer info error: {e}"));
                sleep_with_drain(&mut cmd_rx, &mut current, backoff).await;
                backoff = (backoff * 2).min(60);
                continue;
            }
        };

        match run_once(&token, &info, &quote_tx, &mut cmd_rx, &mut current, &status).await {
            Ok(true) => return, // quit requested
            Ok(false) | Err(_) => {
                set_status(&status, &format!("Disconnected. Reconnecting in {backoff}s..."));
                if sleep_with_drain(&mut cmd_rx, &mut current, backoff).await {
                    return; // quit during sleep
                }
            }
        }
        // Reset backoff after a connection attempt (successful or not) completes
        backoff = (backoff * 2).min(60);
    }
}

// ── Single connection session ─────────────────────────────────────────────────

/// Returns Ok(true) if quit was requested, Ok(false) if disconnected naturally.
async fn run_once(
    token: &str,
    info: &StreamerInfoRaw,
    quote_tx: &mpsc::Sender<QuoteUpdate>,
    cmd_rx: &mut mpsc::Receiver<StreamCommand>,
    current: &mut HashSet<String>,
    status: &Arc<Mutex<String>>,
) -> Result<bool> {
    let (ws, _) = connect_async(&info.streamer_socket_url).await?;
    let (mut write, mut read) = ws.split();

    let mut req_id: u64 = 1;

    // LOGIN
    let login_msg = json!({
        "requests": [{
            "service": "ADMIN",
            "command": "LOGIN",
            "requestid": req_id.to_string(),
            "SchwabClientCustomerId": info.schwab_client_customer_id,
            "SchwabClientCorrelId": info.schwab_client_correl_id,
            "parameters": {
                "Authorization": token,
                "SchwabClientChannel": info.schwab_client_channel,
                "SchwabClientFunctionId": info.schwab_client_function_id,
            }
        }]
    });
    req_id += 1;
    write.send(Message::Text(login_msg.to_string().into())).await?;

    // Wait for LOGIN response
    loop {
        match read.next().await {
            Some(Ok(Message::Text(txt))) => {
                if is_login_ok(&txt) {
                    break;
                }
                if is_login_err(&txt) {
                    bail!("LOGIN rejected by streamer");
                }
            }
            Some(Ok(Message::Ping(d))) => { let _ = write.send(Message::Pong(d)).await; }
            Some(Err(e)) => return Err(e.into()),
            None => return Ok(false),
            _ => {}
        }
    }

    set_status(status, "● Connected");

    // Subscribe to initial symbols
    if !current.is_empty() {
        let keys: Vec<&str> = current.iter().map(String::as_str).collect();
        send_subs(&mut write, &mut req_id, info, &keys).await;
    }

    // Main loop
    loop {
        tokio::select! {
            msg = read.next() => {
                match msg {
                    Some(Ok(Message::Text(txt))) => {
                        parse_data(&txt, quote_tx).await;
                    }
                    Some(Ok(Message::Ping(d))) => { let _ = write.send(Message::Pong(d)).await; }
                    Some(Err(e)) => return Err(e.into()),
                    None => return Ok(false),
                    _ => {}
                }
            }
            cmd = cmd_rx.recv() => {
                match cmd {
                    None | Some(StreamCommand::Quit) => {
                        let _ = send_logout(&mut write, &mut req_id, info).await;
                        return Ok(true);
                    }
                    Some(StreamCommand::SetSymbols(new_syms)) => {
                        let new_set: HashSet<String> = new_syms.into_iter().collect();
                        let to_unsub: Vec<&str> = current.difference(&new_set)
                            .map(String::as_str).collect();
                        let to_sub: Vec<&str> = new_set.difference(current)
                            .map(String::as_str).collect();

                        if !to_unsub.is_empty() {
                            send_unsubs(&mut write, &mut req_id, info, &to_unsub).await;
                        }
                        if !to_sub.is_empty() {
                            send_subs(&mut write, &mut req_id, info, &to_sub).await;
                        }
                        *current = new_set;
                    }
                }
            }
        }
    }
}

// ── Message builders ──────────────────────────────────────────────────────────

async fn send_subs<S>(write: &mut S, req_id: &mut u64, info: &StreamerInfoRaw, keys: &[&str])
where
    S: SinkExt<Message> + Unpin,
    S::Error: std::fmt::Display,
{
    let msg = json!({
        "requests": [{
            "service": "LEVELONE_EQUITIES",
            "command": "SUBS",
            "requestid": req_id.to_string(),
            "SchwabClientCustomerId": info.schwab_client_customer_id,
            "SchwabClientCorrelId": info.schwab_client_correl_id,
            "parameters": {
                "keys": keys.join(","),
                "fields": EQUITY_FIELDS,
            }
        }]
    });
    *req_id += 1;
    let _ = write.send(Message::Text(msg.to_string().into())).await;
}

async fn send_unsubs<S>(write: &mut S, req_id: &mut u64, info: &StreamerInfoRaw, keys: &[&str])
where
    S: SinkExt<Message> + Unpin,
    S::Error: std::fmt::Display,
{
    let msg = json!({
        "requests": [{
            "service": "LEVELONE_EQUITIES",
            "command": "UNSUBS",
            "requestid": req_id.to_string(),
            "SchwabClientCustomerId": info.schwab_client_customer_id,
            "SchwabClientCorrelId": info.schwab_client_correl_id,
            "parameters": { "keys": keys.join(",") }
        }]
    });
    *req_id += 1;
    let _ = write.send(Message::Text(msg.to_string().into())).await;
}

async fn send_logout<S>(write: &mut S, req_id: &mut u64, info: &StreamerInfoRaw) -> Result<()>
where
    S: SinkExt<Message> + Unpin,
    S::Error: std::error::Error + Send + Sync + 'static,
{
    let msg = json!({
        "requests": [{
            "service": "ADMIN",
            "command": "LOGOUT",
            "requestid": req_id.to_string(),
            "SchwabClientCustomerId": info.schwab_client_customer_id,
            "SchwabClientCorrelId": info.schwab_client_correl_id,
        }]
    });
    *req_id += 1;
    write.send(Message::Text(msg.to_string().into())).await?;
    Ok(())
}

// ── Parsing ───────────────────────────────────────────────────────────────────

fn is_login_ok(txt: &str) -> bool {
    let Ok(v) = serde_json::from_str::<Value>(txt) else { return false };
    v["response"][0]["command"].as_str() == Some("LOGIN")
        && v["response"][0]["content"]["code"].as_i64() == Some(0)
}

fn is_login_err(txt: &str) -> bool {
    let Ok(v) = serde_json::from_str::<Value>(txt) else { return false };
    v["response"][0]["command"].as_str() == Some("LOGIN")
        && v["response"][0]["content"]["code"].as_i64() != Some(0)
}

async fn parse_data(txt: &str, quote_tx: &mpsc::Sender<QuoteUpdate>) {
    let Ok(v) = serde_json::from_str::<Value>(txt) else { return };
    let Some(data) = v["data"].as_array() else { return };

    for item in data {
        if item["service"].as_str() != Some("LEVELONE_EQUITIES") { continue; }
        let Some(content) = item["content"].as_array() else { continue };

        for entry in content {
            let Some(symbol) = entry["key"].as_str() else { continue };
            // Only streaming fields we trust (0,1,2,3,8,12,13).
            // All other data comes from REST enrichment via fetch_bulk_quotes.
            let update = QuoteUpdate {
                symbol: symbol.to_string(),
                bid:    entry["1"].as_f64(),
                ask:    entry["2"].as_f64(),
                last:   entry["3"].as_f64(),
                volume: entry["8"].as_u64(),
                high:   entry["12"].as_f64(),
                low:    entry["13"].as_f64(),
                prev_close: None, description: None, open: None,
                net_change: None, w52_high: None, w52_low: None,
                pe_ratio:   None, div_amount: None, div_yield: None,
                eps:        None, market_cap: None,
            };
            let _ = quote_tx.try_send(update);
        }
    }
}

// ── Helpers ───────────────────────────────────────────────────────────────────

fn set_status(status: &Arc<Mutex<String>>, msg: &str) {
    if let Ok(mut s) = status.lock() { *s = msg.to_string(); }
}

/// Sleep for `secs`, draining cmd_rx the whole time. Returns true if Quit received.
async fn sleep_with_drain(
    cmd_rx: &mut mpsc::Receiver<StreamCommand>,
    current: &mut HashSet<String>,
    secs: u64,
) -> bool {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(secs);
    loop {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        if remaining.is_zero() { return false; }
        tokio::select! {
            _ = tokio::time::sleep(remaining) => return false,
            cmd = cmd_rx.recv() => match cmd {
                None | Some(StreamCommand::Quit) => return true,
                Some(StreamCommand::SetSymbols(syms)) => {
                    *current = syms.into_iter().collect();
                }
            }
        }
    }
}
