use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use super::app::{
    AccountSelectPhase, AccountSelectState, AmountEntryState, App, AppScreen, BlacklistFocus,
    BlacklistState, DeleteTarget, IndexLookupPhase, IndexLookupState, Mode, NewListFocus, NewListState,
    PlanRequest, PlanReviewPhase, PlanReviewState, PriceInputFocus, PriceInputState, SubmitRequest, SubmitState,
};

/// Handle a key event. Returns true if the app should quit.
pub fn handle_key(key: KeyEvent, app: &mut App) {
    match &app.screen {
        AppScreen::Watchlists => handle_watchlists_screen(key, app),
        AppScreen::IndexLookup(_) => handle_index_lookup(key, app),
        AppScreen::AccountSelect(_) => handle_account_select(key, app),
        AppScreen::AmountEntry(_) => handle_amount_entry(key, app),
        AppScreen::PlanReview(_) => handle_plan_review(key, app),
        AppScreen::Submitting(_) => handle_submitting(key, app),
        AppScreen::PriceInput(_) => handle_price_input(key, app),
        AppScreen::PriceGrid(_) => handle_price_grid(key, app),
    }
}

fn handle_watchlists_screen(key: KeyEvent, app: &mut App) {
    match &app.mode {
        Mode::Normal => handle_normal(key, app),
        Mode::AddSymbol(_) => handle_add_symbol(key, app),
        Mode::NewList(_) => handle_new_list(key, app),
        Mode::ConfirmDelete(_) => handle_confirm_delete(key, app),
        Mode::Blacklist(_) => handle_blacklist(key, app),
        Mode::Detail => {
            // Any key closes the detail popup
            app.mode = Mode::Normal;
        }
    }
}

// ── Normal mode ────────────────────────────────────────────────────────────────

fn handle_normal(key: KeyEvent, app: &mut App) {
    match key.code {
        // Quit
        KeyCode::Char('q') | KeyCode::Char('Q') => app.should_quit = true,
        KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => app.should_quit = true,

        // Row navigation
        KeyCode::Char('j') | KeyCode::Down  => app.nav_down(),
        KeyCode::Char('k') | KeyCode::Up    => app.nav_up(),
        KeyCode::PageDown                   => app.page_down(),
        KeyCode::PageUp                     => app.page_up(),

        // Tab switching — Tab/Shift-Tab and h/l and Left/Right
        KeyCode::Tab | KeyCode::Char('l') | KeyCode::Right => {
            if !app.watchlists.is_empty() {
                let next = (app.active_tab + 1) % app.watchlists.len();
                app.switch_tab(next);
            }
        }
        KeyCode::BackTab | KeyCode::Char('h') | KeyCode::Left => {
            if !app.watchlists.is_empty() {
                let prev = if app.active_tab == 0 {
                    app.watchlists.len() - 1
                } else {
                    app.active_tab - 1
                };
                app.switch_tab(prev);
            }
        }
        // Jump to tab 1–9
        KeyCode::Char(c @ '1'..='9') => {
            let idx = (c as usize) - ('1' as usize);
            app.switch_tab(idx);
        }

        // Detail popup for selected symbol
        KeyCode::Enter => {
            if app.table_state.selected().is_some() {
                app.mode = Mode::Detail;
            }
        }

        // Add symbol
        KeyCode::Char('a') => {
            app.mode = Mode::AddSymbol(String::new());
        }

        // Delete selected symbol
        KeyCode::Char('d') | KeyCode::Delete => {
            if app.table_state.selected().is_some() {
                app.mode = Mode::ConfirmDelete(DeleteTarget::Symbol);
            }
        }

        // New watchlist
        KeyCode::Char('n') => {
            app.mode = Mode::NewList(NewListState::new());
        }

        // Delete active watchlist (shift-D)
        KeyCode::Char('D') => {
            if !app.watchlists.is_empty() {
                app.mode = Mode::ConfirmDelete(DeleteTarget::List);
            }
        }

        // Manage global trading blacklist
        KeyCode::Char('b') => {
            app.mode = Mode::Blacklist(BlacklistState::new(app.blacklist.len()));
        }

        // Look up an index's weighted constituents
        KeyCode::Char('i') => {
            app.screen = AppScreen::IndexLookup(IndexLookupState::new());
        }

        // Pick which Schwab account to invest from
        KeyCode::Char('A') => {
            app.screen = AppScreen::AccountSelect(AccountSelectState::new());
            let _ = app.accounts_tx.try_send(());
        }

        // Options pricing calculator (Black-Scholes grid)
        KeyCode::Char('P') => {
            app.screen = AppScreen::PriceInput(PriceInputState::new());
        }

        _ => {}
    }
}

// ── Add symbol mode ────────────────────────────────────────────────────────────

fn handle_add_symbol(key: KeyEvent, app: &mut App) {
    let Mode::AddSymbol(ref mut input) = app.mode else { return };
    match key.code {
        KeyCode::Esc => { app.mode = Mode::Normal; }
        KeyCode::Enter => {
            let sym = input.clone();
            app.mode = Mode::Normal;
            app.add_symbol(sym);
        }
        KeyCode::Backspace => { input.pop(); }
        KeyCode::Char(c) if c.is_alphanumeric() || c == '.' || c == '/' || c == '-' => {
            if input.len() < 12 { input.push(c.to_ascii_uppercase()); }
        }
        _ => {}
    }
}

// ── New list mode ──────────────────────────────────────────────────────────────

fn handle_new_list(key: KeyEvent, app: &mut App) {
    let Mode::NewList(ref mut state) = app.mode else { return };
    let num_sources = NewListState::sources().len();

    match key.code {
        KeyCode::Esc => { app.mode = Mode::Normal; }

        KeyCode::Tab => {
            state.focus = match state.focus {
                NewListFocus::Name   => NewListFocus::Source,
                NewListFocus::Source => NewListFocus::Name,
            };
        }

        KeyCode::Enter => {
            if state.name.trim().is_empty() {
                state.focus = NewListFocus::Name;
                return;
            }
            // Move state out of app.mode
            let Mode::NewList(state) = std::mem::replace(&mut app.mode, Mode::Normal) else { return };
            app.create_list(state);
        }

        // Arrow keys always navigate the source list regardless of focus field
        KeyCode::Down => {
            let idx = (state.source_idx + 1) % num_sources;
            state.set_source(idx);
        }
        KeyCode::Up => {
            let idx = if state.source_idx == 0 { num_sources - 1 } else { state.source_idx - 1 };
            state.set_source(idx);
        }
        // j/k only in Source focus (avoid eating name input characters)
        KeyCode::Char('j') if state.focus == NewListFocus::Source => {
            let idx = (state.source_idx + 1) % num_sources;
            state.set_source(idx);
        }
        KeyCode::Char('k') if state.focus == NewListFocus::Source => {
            let idx = if state.source_idx == 0 { num_sources - 1 } else { state.source_idx - 1 };
            state.set_source(idx);
        }

        // Name typing — mark as manually edited so auto-fill stops overwriting it
        KeyCode::Backspace if state.focus == NewListFocus::Name => {
            state.name.pop();
            state.name_edited = !state.name.is_empty();
        }
        KeyCode::Char(c) if state.focus == NewListFocus::Name => {
            if state.name.len() < 24 {
                state.name.push(c);
                state.name_edited = true;
            }
        }

        _ => {}
    }
}

// ── Blacklist mode ─────────────────────────────────────────────────────────────

fn handle_blacklist(key: KeyEvent, app: &mut App) {
    let in_add = matches!(app.mode, Mode::Blacklist(BlacklistState { focus: BlacklistFocus::Add(_), .. }));
    let in_confirm = matches!(app.mode, Mode::Blacklist(BlacklistState { focus: BlacklistFocus::ConfirmDelete, .. }));
    if in_add {
        handle_blacklist_add(key, app);
    } else if in_confirm {
        handle_blacklist_confirm_delete(key, app);
    } else {
        handle_blacklist_view(key, app);
    }
}

fn handle_blacklist_view(key: KeyEvent, app: &mut App) {
    let len = app.blacklist.len();
    let Mode::Blacklist(ref mut state) = app.mode else { return };
    match key.code {
        KeyCode::Esc | KeyCode::Char('b') => { app.mode = Mode::Normal; }
        KeyCode::Char('j') | KeyCode::Down => {
            if len > 0 {
                state.selected = Some(state.selected.map(|i| (i + 1) % len).unwrap_or(0));
            }
        }
        KeyCode::Char('k') | KeyCode::Up => {
            if len > 0 {
                let i = match state.selected {
                    Some(i) if i > 0 => i - 1,
                    _ => len - 1,
                };
                state.selected = Some(i);
            }
        }
        KeyCode::Char('a') => { state.focus = BlacklistFocus::Add(String::new()); }
        KeyCode::Char('d') | KeyCode::Delete => {
            if state.selected.is_some() { state.focus = BlacklistFocus::ConfirmDelete; }
        }
        _ => {}
    }
}

fn handle_blacklist_add(key: KeyEvent, app: &mut App) {
    let Mode::Blacklist(ref mut state) = app.mode else { return };
    let BlacklistFocus::Add(ref mut input) = state.focus else { return };
    match key.code {
        KeyCode::Esc => { state.focus = BlacklistFocus::View; }
        KeyCode::Enter => {
            let Mode::Blacklist(state) = std::mem::replace(&mut app.mode, Mode::Normal) else { return };
            let BlacklistFocus::Add(sym) = state.focus else { return };
            app.add_blacklist_symbol(sym);
            let selected = if app.blacklist.is_empty() { None } else { Some(0) };
            app.mode = Mode::Blacklist(BlacklistState { selected, focus: BlacklistFocus::View });
        }
        KeyCode::Backspace => { input.pop(); }
        KeyCode::Char(c) if c.is_alphanumeric() || c == '.' || c == '/' || c == '-' => {
            if input.len() < 12 { input.push(c.to_ascii_uppercase()); }
        }
        _ => {}
    }
}

// ── Index lookup screen ─────────────────────────────────────────────────────────

fn handle_index_lookup(key: KeyEvent, app: &mut App) {
    let is_input = matches!(app.screen, AppScreen::IndexLookup(IndexLookupState { phase: IndexLookupPhase::Input, .. }));
    let is_loading = matches!(app.screen, AppScreen::IndexLookup(IndexLookupState { phase: IndexLookupPhase::Loading, .. }));
    if is_input {
        handle_index_lookup_input(key, app);
    } else if is_loading {
        if key.code == KeyCode::Esc { app.screen = AppScreen::Watchlists; }
    } else {
        handle_index_lookup_result(key, app);
    }
}

fn handle_index_lookup_input(key: KeyEvent, app: &mut App) {
    let AppScreen::IndexLookup(ref mut state) = app.screen else { return };
    match key.code {
        KeyCode::Esc => { app.screen = AppScreen::Watchlists; }
        KeyCode::Enter => {
            let query = state.query.trim().to_string();
            if query.is_empty() { return; }
            state.phase = IndexLookupPhase::Loading;
            let _ = app.lookup_tx.try_send(query);
        }
        KeyCode::Backspace => { state.query.pop(); }
        KeyCode::Char(c) if c.is_alphanumeric() || c == '&' || c == ' ' => {
            if state.query.len() < 24 { state.query.push(c.to_ascii_uppercase()); }
        }
        _ => {}
    }
}

fn handle_index_lookup_result(key: KeyEvent, app: &mut App) {
    match key.code {
        KeyCode::Esc => { app.screen = AppScreen::Watchlists; }
        KeyCode::Char('n') => { app.screen = AppScreen::IndexLookup(IndexLookupState::new()); }
        KeyCode::Char('j') | KeyCode::Down => app.nav_lookup_down(),
        KeyCode::Char('k') | KeyCode::Up => app.nav_lookup_up(),
        KeyCode::Char('p') => start_amount_entry(app),
        _ => {}
    }
}

/// From a loaded IndexLookup, carry the resolved entry+holdings forward into
/// AmountEntry. No-op if the lookup isn't in the Loaded phase.
fn start_amount_entry(app: &mut App) {
    let AppScreen::IndexLookup(ref state) = app.screen else { return };
    let IndexLookupPhase::Loaded { ref entry, ref holdings, .. } = state.phase else { return };
    app.screen = AppScreen::AmountEntry(AmountEntryState::new(entry.clone(), holdings.clone()));
}

// ── Account select screen ───────────────────────────────────────────────────────

fn handle_account_select(key: KeyEvent, app: &mut App) {
    let is_loaded = matches!(
        app.screen,
        AppScreen::AccountSelect(AccountSelectState { phase: AccountSelectPhase::Loaded { .. } })
    );
    match key.code {
        KeyCode::Esc => { app.screen = AppScreen::Watchlists; }
        KeyCode::Char('j') | KeyCode::Down if is_loaded => app.nav_account_down(),
        KeyCode::Char('k') | KeyCode::Up if is_loaded => app.nav_account_up(),
        KeyCode::Enter if is_loaded => app.confirm_account_selection(),
        _ => {}
    }
}

// ── Amount entry screen ──────────────────────────────────────────────────────────

fn handle_amount_entry(key: KeyEvent, app: &mut App) {
    let AppScreen::AmountEntry(ref mut state) = app.screen else { return };
    match key.code {
        KeyCode::Esc => { app.screen = AppScreen::Watchlists; }
        KeyCode::Enter => {
            let Some(account) = app.selected_account.as_ref() else {
                state.error = Some("Select an account first — press 'A' from Watchlists.".to_string());
                return;
            };
            let Ok(amount) = state.amount_input.trim().parse::<f64>() else {
                state.error = Some("Enter a valid dollar amount.".to_string());
                return;
            };
            if amount <= 0.0 {
                state.error = Some("Amount must be greater than $0.".to_string());
                return;
            }

            let request = PlanRequest {
                index_name: state.entry.name.clone(),
                account_hash: account.hash_value.clone(),
                holdings: state.holdings.clone(),
                blacklist: app.blacklist.clone(),
                total_amount: amount,
            };
            app.screen = AppScreen::PlanReview(PlanReviewState::new());
            let _ = app.plan_tx.try_send(request);
        }
        KeyCode::Backspace => { state.amount_input.pop(); state.error = None; }
        KeyCode::Char(c) if c.is_ascii_digit() || c == '.' => {
            if state.amount_input.len() < 12 { state.amount_input.push(c); }
            state.error = None;
        }
        _ => {}
    }
}

// ── Plan review screen ───────────────────────────────────────────────────────────

fn handle_plan_review(key: KeyEvent, app: &mut App) {
    let is_loaded = matches!(
        app.screen,
        AppScreen::PlanReview(PlanReviewState { phase: PlanReviewPhase::Loaded { .. } })
    );
    match key.code {
        KeyCode::Esc => { app.screen = AppScreen::Watchlists; }
        KeyCode::Char('j') | KeyCode::Down if is_loaded => app.nav_plan_down(),
        KeyCode::Char('k') | KeyCode::Up if is_loaded => app.nav_plan_up(),
        // Dry-run only: builds and shows the order JSON that WOULD be sent,
        // without sending anything. Live submission isn't wired to any key
        // yet — see SubmitRequest's doc comment.
        KeyCode::Char('s') if is_loaded => start_dry_run_submit(app),
        _ => {}
    }
}

fn start_dry_run_submit(app: &mut App) {
    let AppScreen::PlanReview(ref state) = app.screen else { return };
    let PlanReviewPhase::Loaded { ref plan, .. } = state.phase else { return };
    let request = SubmitRequest { plan: plan.clone(), dry_run: true };
    app.screen = AppScreen::Submitting(SubmitState::new());
    let _ = app.submit_tx.try_send(request);
}

// ── Submitting screen ─────────────────────────────────────────────────────────────

fn handle_submitting(key: KeyEvent, app: &mut App) {
    let viewing = match &app.screen {
        AppScreen::Submitting(SubmitState { phase: super::app::SubmitPhase::Done { viewing, .. } }) => *viewing,
        _ => None,
    };
    match key.code {
        KeyCode::Esc => {
            if viewing.is_some() {
                app.toggle_submit_detail();
            } else {
                app.screen = AppScreen::Watchlists;
            }
        }
        KeyCode::Char('j') | KeyCode::Down if viewing.is_none() => app.nav_submit_down(),
        KeyCode::Char('k') | KeyCode::Up if viewing.is_none() => app.nav_submit_up(),
        KeyCode::Enter | KeyCode::Char('v') => app.toggle_submit_detail(),
        _ => {}
    }
}

fn handle_blacklist_confirm_delete(key: KeyEvent, app: &mut App) {
    let idx = match &app.mode {
        Mode::Blacklist(state) => state.selected,
        _ => return,
    };
    match key.code {
        KeyCode::Char('y') | KeyCode::Enter => {
            let new_sel = idx.and_then(|i| app.delete_blacklist_symbol(i));
            app.mode = Mode::Blacklist(BlacklistState { selected: new_sel, focus: BlacklistFocus::View });
        }
        KeyCode::Char('n') | KeyCode::Esc => {
            app.mode = Mode::Blacklist(BlacklistState { selected: idx, focus: BlacklistFocus::View });
        }
        _ => {}
    }
}

// ── Confirm delete mode ────────────────────────────────────────────────────────

fn handle_confirm_delete(key: KeyEvent, app: &mut App) {
    let is_symbol = matches!(app.mode, Mode::ConfirmDelete(DeleteTarget::Symbol));
    let is_list   = matches!(app.mode, Mode::ConfirmDelete(DeleteTarget::List));
    match key.code {
        KeyCode::Char('y') | KeyCode::Enter => {
            app.mode = Mode::Normal;
            if is_symbol      { app.delete_selected_symbol(); }
            else if is_list   { app.delete_active_list(); }
        }
        KeyCode::Char('n') | KeyCode::Esc => { app.mode = Mode::Normal; }
        _ => {}
    }
}

// ── Price input / grid screens ────────────────────────────────────────────────

fn handle_price_input(key: KeyEvent, app: &mut App) {
    let AppScreen::PriceInput(ref mut state) = app.screen else { return };

    match key.code {
        KeyCode::Esc => { app.screen = AppScreen::Watchlists; }

        KeyCode::Tab => {
            state.focus = match state.focus {
                PriceInputFocus::Symbol => PriceInputFocus::Expiry,
                PriceInputFocus::Expiry => PriceInputFocus::Iv,
                PriceInputFocus::Iv => PriceInputFocus::OptionType,
                PriceInputFocus::OptionType => PriceInputFocus::Rate,
                PriceInputFocus::Rate => PriceInputFocus::DividendYield,
                PriceInputFocus::DividendYield => PriceInputFocus::Symbol,
            };
        }
        KeyCode::BackTab => {
            state.focus = match state.focus {
                PriceInputFocus::Symbol => PriceInputFocus::DividendYield,
                PriceInputFocus::Expiry => PriceInputFocus::Symbol,
                PriceInputFocus::Iv => PriceInputFocus::Expiry,
                PriceInputFocus::OptionType => PriceInputFocus::Iv,
                PriceInputFocus::Rate => PriceInputFocus::OptionType,
                PriceInputFocus::DividendYield => PriceInputFocus::Rate,
            };
        }

        KeyCode::Enter => match state.build_request() {
            Ok(req) => {
                app.screen = AppScreen::PriceGrid(super::app::PriceGridState::new());
                let _ = app.price_tx.try_send(req);
            }
            Err(msg) => { state.error = Some(msg); }
        },

        // Call/Put toggle — only when that field is focused.
        KeyCode::Left | KeyCode::Right | KeyCode::Up | KeyCode::Down
            if state.focus == PriceInputFocus::OptionType =>
        {
            state.option_type_idx = 1 - state.option_type_idx;
        }

        KeyCode::Backspace => {
            match state.focus {
                PriceInputFocus::Symbol => { state.symbol.pop(); }
                PriceInputFocus::Expiry => { state.expiry.pop(); }
                PriceInputFocus::Iv => { state.iv.pop(); }
                PriceInputFocus::Rate => { state.rate.pop(); }
                PriceInputFocus::DividendYield => { state.dividend_yield.pop(); }
                PriceInputFocus::OptionType => {}
            }
            state.error = None;
        }
        KeyCode::Char(c) => {
            match state.focus {
                PriceInputFocus::Symbol if c.is_alphanumeric() => {
                    if state.symbol.len() < 12 { state.symbol.push(c.to_ascii_uppercase()); }
                }
                PriceInputFocus::Expiry if c.is_ascii_digit() || c == '-' => {
                    if state.expiry.len() < 10 { state.expiry.push(c); }
                }
                PriceInputFocus::Iv if c.is_ascii_digit() || c == '.' => {
                    if state.iv.len() < 10 { state.iv.push(c); }
                }
                PriceInputFocus::Rate if c.is_ascii_digit() || c == '.' => {
                    if state.rate.len() < 10 { state.rate.push(c); }
                }
                PriceInputFocus::DividendYield if c.is_ascii_digit() || c == '.' => {
                    if state.dividend_yield.len() < 10 { state.dividend_yield.push(c); }
                }
                _ => {}
            }
            state.error = None;
        }
        _ => {}
    }
}

fn handle_price_grid(key: KeyEvent, app: &mut App) {
    if key.code == KeyCode::Esc {
        app.screen = AppScreen::Watchlists;
    }
}
