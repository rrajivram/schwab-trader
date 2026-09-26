//! Live checks against SSGA + Schwab for schwab-indexer's data layer.
//! Needs a valid login; run with `cargo test --test live_indexer -- --ignored --nocapture`.

use schwab::{accounts, api, auth, dividends, portfolio, universe};

#[tokio::test]
#[ignore]
async fn market_data_covers_the_universe() {
    let u = universe::load(false).await.unwrap();
    let symbols: Vec<String> = u.constituents.iter().map(|c| c.symbol.clone()).collect();
    let token = auth::get_valid_token().await.unwrap();
    let md = api::fetch_market_data(&token, &symbols).await.unwrap();

    let missing: Vec<&String> = symbols.iter().filter(|s| !md.contains_key(*s)).collect();
    println!("{} / {} symbols quoted; missing: {missing:?}", md.len(), symbols.len());
    assert!(missing.is_empty(), "every constituent should quote");
    assert!(md["BRK.B"].price.is_some(), "slash-form mapping for BRK.B");
    let no_yield = md.values().filter(|m| m.div_yield.is_none()).count();
    println!("{no_yield} without a div yield field");
}

#[tokio::test]
#[ignore]
async fn dividends_attach_to_held_positions() {
    let hashes = accounts::list_account_numbers().await.unwrap();
    let acct = accounts::get_account(&hashes[0]).await.unwrap();
    let held = portfolio::held_positions(&acct, &Default::default());
    let symbols: Vec<String> = held.keys().cloned().collect();
    let divs = dividends::dividends_by_symbol(&acct.hash_value, &symbols).await.unwrap();
    println!("held {symbols:?}, dividends {divs:?}");
}

#[tokio::test]
#[ignore]
async fn auto_basket_on_live_data() {
    let u = universe::load(false).await.unwrap();
    let symbols: Vec<String> = u.constituents.iter().map(|c| c.symbol.clone()).collect();
    let token = auth::get_valid_token().await.unwrap();
    let md = api::fetch_market_data(&token, &symbols).await.unwrap();
    let eps = md.iter().filter_map(|(s, m)| Some((s.clone(), m.eps?))).collect();
    let picks = schwab::basket::auto_basket(&u, &eps, &Default::default(), 13);
    for p in &picks {
        println!("{:<6} {:<24} EPS {:>7.2}  weight {:.3}%", p.symbol, p.sector, p.score.unwrap_or(f64::NAN), p.weight * 100.0);
    }
    assert_eq!(picks.len(), 13);
}

/// Full order flow in PREVIEW mode: live account lookup + cash check, a
/// sell and buys built into request bodies. Nothing is sent to Schwab.
#[tokio::test]
#[ignore]
async fn preview_order_run_sends_nothing() {
    use schwab::execution::{self, ExecEvent, OrderState, Plan, PlannedOrder};
    use schwab::orders::Side;
    use std::sync::{atomic::AtomicBool, Arc, Mutex};

    let order = |side, symbol: &str, schwab: &str, qty, price| PlannedOrder {
        side,
        symbol: symbol.into(),
        schwab_symbol: schwab.into(),
        quantity: qty,
        est_price: price,
    };
    let plan = Plan {
        preview: true,
        sells: vec![order(Side::Sell, "F", "F", 1.0, 12.0)],
        buys: vec![order(Side::Buy, "BRK.B", "BRK/B", 0.0123, 480.0), order(Side::Buy, "KO", "KO", 0.05, 87.0)],
        amount: 0.0123 * 480.0 + 0.05 * 87.0,
        planned_proceeds: 12.0,
        do_not_transact: Default::default(),
    };
    let events = Arc::new(Mutex::new(Vec::new()));
    let sink = events.clone();
    execution::run(plan, Arc::new(AtomicBool::new(false)), move |e| sink.lock().unwrap().push(e)).await;

    let events = events.lock().unwrap();
    for e in events.iter() {
        match e {
            ExecEvent::Sell(_, OrderState::Preview(b)) | ExecEvent::Buy(_, OrderState::Preview(b)) => println!("{b}"),
            other => println!("{other:?}"),
        }
    }
    let previews = events
        .iter()
        .filter(|e| matches!(e, ExecEvent::Sell(_, OrderState::Preview(_)) | ExecEvent::Buy(_, OrderState::Preview(_))))
        .count();
    assert_eq!(previews, 3, "one sell + two buys, all previews");
    assert!(!events.iter().any(|e| matches!(e, ExecEvent::Sell(_, OrderState::Placed { .. }) | ExecEvent::Buy(_, OrderState::Placed { .. }))));
}
