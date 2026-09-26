//! Places a basket's orders: sells first (rebalance), wait for them to
//! fill, then buys scaled to the actual proceeds. MARKET/DAY orders only.
//! In preview mode nothing is sent — every order resolves to the request
//! body that would have been posted, and sells are assumed to fill at their
//! estimated price.

use anyhow::{bail, Context, Result};
use chrono::Utc;
use serde_json::{json, Value};
use std::collections::HashSet;
use std::io::Write;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use crate::accounts;
use crate::orders::{self, OrderOutcome, OrderStatus, Side};

const POLL_INTERVAL: Duration = Duration::from_secs(3);

#[derive(Debug, Clone, PartialEq)]
pub struct PlannedOrder {
    pub side: Side,
    /// Ticker as the indexer shows it (`BRK.B`, `GOOGL`).
    pub symbol: String,
    /// Ticker as sent to Schwab (`BRK/B`; for sells, the position's own symbol).
    pub schwab_symbol: String,
    pub quantity: f64,
    pub est_price: f64,
}

impl PlannedOrder {
    pub fn est_value(&self) -> f64 {
        self.quantity * self.est_price
    }
}

pub struct Plan {
    pub preview: bool,
    pub sells: Vec<PlannedOrder>,
    pub buys: Vec<PlannedOrder>,
    /// Total to invest in buys.
    pub amount: f64,
    /// Expected sale proceeds included in `amount` (0 when creating).
    pub planned_proceeds: f64,
    /// Final guard: nothing on this list is ever sent.
    pub do_not_transact: HashSet<String>,
}

#[derive(Debug, Clone)]
pub enum OrderState {
    Pending,
    /// Preview mode: the body that would have been posted.
    Preview(Value),
    /// Live: accepted by Schwab, latest known status.
    Placed { location: Option<String>, status: Option<OrderStatus> },
    Failed(String),
}

#[derive(Debug, Clone)]
pub enum ExecEvent {
    /// Buys after scaling to actual proceeds; replaces the planned buy list.
    BuysPlanned(Vec<PlannedOrder>),
    Sell(usize, OrderState),
    Buy(usize, OrderState),
    Note(String),
    /// Done — either everything reached a final state, or the user stopped
    /// waiting, or a pre-flight check failed.
    Finished,
}

/// Buys after sells: if proceeds came in below plan, shrink the amount by
/// the shortfall and scale every buy proportionally (never scaled up).
/// Quantities are floored to 4 decimals; zero-quantity buys are dropped.
pub fn scale_buys(buys: &[PlannedOrder], amount: f64, planned_proceeds: f64, actual_proceeds: f64) -> Vec<PlannedOrder> {
    let shortfall = (planned_proceeds - actual_proceeds).max(0.0);
    let factor = if amount > 0.0 { ((amount - shortfall) / amount).clamp(0.0, 1.0) } else { 0.0 };
    buys.iter()
        .map(|b| PlannedOrder { quantity: (b.quantity * factor * 10_000.0).floor() / 10_000.0, ..b.clone() })
        .filter(|b| b.quantity > 0.0)
        .collect()
}

/// Checks that don't need the network.
pub fn validate(plan: &Plan) -> Result<()> {
    for o in plan.sells.iter().chain(&plan.buys) {
        if plan.do_not_transact.contains(&o.symbol) {
            bail!("{} is on the do-not-transact list — nothing was sent", o.symbol);
        }
        if !(o.quantity > 0.0) {
            bail!("{} has no quantity — nothing was sent", o.symbol);
        }
    }
    Ok(())
}

pub async fn run(plan: Plan, cancel: Arc<AtomicBool>, emit: impl Fn(ExecEvent) + Send + Sync) {
    if let Err(e) = run_inner(&plan, &cancel, &emit).await {
        emit(ExecEvent::Note(format!("Stopped: {e}")));
    }
    emit(ExecEvent::Finished);
}

async fn run_inner(plan: &Plan, cancel: &AtomicBool, emit: &(impl Fn(ExecEvent) + Send + Sync)) -> Result<()> {
    validate(plan)?;

    // Account hashes go stale, so always resolve it live right before trading.
    let hashes = accounts::list_account_numbers().await?;
    let hash = hashes.first().context("This token has no linked accounts")?;
    let account = accounts::get_account(hash).await?;
    let cash_needed = plan.amount - plan.planned_proceeds;
    if cash_needed > account.cash_balance + 0.01 {
        bail!(
            "needs ${cash_needed:.2} of cash beyond sale proceeds but the account has ${:.2}",
            account.cash_balance
        );
    }

    // Sells.
    let sell_states = place_all(plan, &hash.hash_value, &plan.sells, cancel, emit, ExecEvent::Sell).await;
    if cancel.load(Ordering::Relaxed) {
        emit(ExecEvent::Note("Stopped waiting for sells — no buys were placed.".into()));
        return Ok(());
    }

    let actual_proceeds = if plan.preview {
        plan.planned_proceeds
    } else {
        sell_states
            .iter()
            .filter_map(|s| match s {
                OrderState::Placed { status: Some(st), .. } => Some(st.fill_value),
                _ => None,
            })
            .sum()
    };

    // Buys, scaled to what the sells actually raised.
    let buys = scale_buys(&plan.buys, plan.amount, plan.planned_proceeds, actual_proceeds);
    if !plan.sells.is_empty() {
        emit(ExecEvent::Note(format!(
            "Sells raised ${actual_proceeds:.2} (planned ${:.2}).",
            plan.planned_proceeds
        )));
    }
    if buys.len() < plan.buys.len() || actual_proceeds + 0.005 < plan.planned_proceeds {
        emit(ExecEvent::Note("Buys were scaled down to match the actual sale proceeds.".into()));
    }
    emit(ExecEvent::BuysPlanned(buys.clone()));
    place_all(plan, &hash.hash_value, &buys, cancel, emit, ExecEvent::Buy).await;
    if cancel.load(Ordering::Relaxed) {
        emit(ExecEvent::Note("Stopped tracking — placed orders stay working at Schwab.".into()));
    }
    Ok(())
}

/// Submit every order, then poll the live ones until all are final or the
/// user cancels. Returns each order's last known state.
async fn place_all(
    plan: &Plan,
    account_hash: &str,
    orders: &[PlannedOrder],
    cancel: &AtomicBool,
    emit: &(impl Fn(ExecEvent) + Send + Sync),
    event: fn(usize, OrderState) -> ExecEvent,
) -> Vec<OrderState> {
    let mut states = Vec::with_capacity(orders.len());
    for (i, o) in orders.iter().enumerate() {
        let state = match orders::submit_equity(account_hash, o.side, &o.schwab_symbol, o.quantity, plan.preview).await {
            Ok(OrderOutcome::DryRun { request_body }) => OrderState::Preview(request_body),
            Ok(OrderOutcome::Submitted { order_location }) => OrderState::Placed { location: order_location, status: None },
            Ok(OrderOutcome::Rejected { status, body }) => OrderState::Failed(format!("Rejected ({status}): {body}")),
            Err(e) => OrderState::Failed(e.to_string()),
        };
        if !plan.preview {
            log_order(o, &state);
        }
        emit(event(i, state.clone()));
        states.push(state);
    }

    loop {
        let open: Vec<usize> = states
            .iter()
            .enumerate()
            .filter(|(_, s)| match s {
                OrderState::Placed { location: Some(_), status } => !status.as_ref().is_some_and(OrderStatus::is_terminal),
                _ => false,
            })
            .map(|(i, _)| i)
            .collect();
        if open.is_empty() || cancel.load(Ordering::Relaxed) {
            break;
        }
        tokio::time::sleep(POLL_INTERVAL).await;
        for i in open {
            let OrderState::Placed { location: Some(loc), status } = &mut states[i] else { continue };
            // A failed poll is transient; keep the last known status and retry.
            if let Ok(st) = orders::get_order_status(loc).await {
                if st.is_terminal() {
                    log_status(&orders[i], &st);
                }
                *status = Some(st);
                emit(event(i, states[i].clone()));
            }
        }
    }
    states
}

fn log_path() -> Result<PathBuf> {
    let dir = dirs::config_dir().context("Cannot locate config directory")?.join("schwab-cli");
    std::fs::create_dir_all(&dir)?;
    Ok(dir.join("order-log.jsonl"))
}

fn append_log(entry: Value) {
    let write = || -> Result<()> {
        let mut f = std::fs::OpenOptions::new().create(true).append(true).open(log_path()?)?;
        writeln!(f, "{entry}")?;
        Ok(())
    };
    // Logging must never block trading; a failure here is only a lost record.
    let _ = write();
}

fn log_order(o: &PlannedOrder, state: &OrderState) {
    let (result, detail) = match state {
        OrderState::Placed { location, .. } => ("placed", json!(location)),
        OrderState::Failed(e) => ("failed", json!(e)),
        OrderState::Preview(_) | OrderState::Pending => return,
    };
    append_log(json!({
        "time": Utc::now().to_rfc3339(),
        "event": "submit",
        "side": o.side.instruction(),
        "symbol": o.schwab_symbol,
        "quantity": o.quantity,
        "est_price": o.est_price,
        "result": result,
        "detail": detail,
    }));
}

fn log_status(o: &PlannedOrder, st: &OrderStatus) {
    append_log(json!({
        "time": Utc::now().to_rfc3339(),
        "event": "final",
        "side": o.side.instruction(),
        "symbol": o.schwab_symbol,
        "status": st.status,
        "filled_quantity": st.filled_quantity,
        "avg_price": st.avg_price(),
        "description": st.description,
    }));
}

#[cfg(test)]
mod tests {
    use super::*;

    fn buy(symbol: &str, qty: f64, price: f64) -> PlannedOrder {
        PlannedOrder { side: Side::Buy, symbol: symbol.into(), schwab_symbol: symbol.into(), quantity: qty, est_price: price }
    }

    #[test]
    fn buys_unchanged_when_proceeds_meet_plan() {
        let b = [buy("A", 2.5, 10.0)];
        assert_eq!(scale_buys(&b, 25.0, 25.0, 30.0), b.to_vec());
    }

    #[test]
    fn shortfall_scales_buys_down_proportionally() {
        // $100 plan funded by $80 of sales; sales only raised $60 → $80 left, ×0.8.
        let b = [buy("A", 5.0, 10.0), buy("B", 1.0, 50.0)];
        let s = scale_buys(&b, 100.0, 80.0, 60.0);
        assert_eq!(s[0].quantity, 4.0);
        assert_eq!(s[1].quantity, 0.8);
    }

    #[test]
    fn nothing_raised_and_no_cash_drops_every_buy() {
        let b = [buy("A", 5.0, 10.0)];
        assert!(scale_buys(&b, 50.0, 50.0, 0.0).is_empty());
    }

    #[test]
    fn do_not_transact_blocks_the_whole_plan() {
        let plan = Plan {
            preview: true,
            sells: vec![],
            buys: vec![buy("A", 1.0, 10.0), buy("B", 1.0, 10.0)],
            amount: 20.0,
            planned_proceeds: 0.0,
            do_not_transact: HashSet::from(["B".to_string()]),
        };
        assert!(validate(&plan).unwrap_err().to_string().contains("B is on the do-not-transact list"));
    }
}
