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
