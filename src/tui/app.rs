use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Instant;

use chrono::NaiveDate;
use ratatui::widgets::TableState;
use tokio::sync::mpsc;

use crate::accounts::Account;
use crate::indices::Index;
use crate::orders::OrderResult;
use crate::pricing::{OptionType, PriceGrid};
use crate::rebalance::RebalancePlan;
use crate::registry::{holdings::CachedHoldings, RegistryEntry};
use crate::stream::{QuoteUpdate, StreamCommand};
use crate::watchlist::Watchlist;

// ── Quote data ─────────────────────────────────────────────────────────────────

#[derive(Default, Clone)]
pub struct Quote {
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
    pub updated: Option<Instant>,
}

impl Quote {
    /// Real-time change: live last minus REST prev_close is most accurate.
    pub fn change(&self) -> Option<f64> {
        if let (Some(last), Some(prev)) = (self.last, self.prev_close) {
            Some(last - prev)
        } else {
            self.net_change
        }
    }

    pub fn pct_change(&self) -> Option<f64> {
        let last = self.last?;
        let close = self.prev_close.filter(|&c| c != 0.0)?;
        Some((last - close) / close * 100.0)
    }
}

// ── UI modes ───────────────────────────────────────────────────────────────────

pub struct NewListState {
    pub name: String,
    pub source_idx: usize, // 0=Empty, 1=SP500, 2=NASDAQ100, 3=DOW30
    pub focus: NewListFocus,
    pub name_edited: bool, // true once user has manually typed in the name field
}

#[derive(PartialEq)]
pub enum NewListFocus {
    Name,
    Source,
}

impl NewListState {
    pub fn new() -> Self {
        Self { name: String::new(), source_idx: 0, focus: NewListFocus::Name, name_edited: false }
    }

    pub fn sources() -> &'static [&'static str] {
        &["Empty", "S&P 500", "NASDAQ 100", "DOW 30"]
    }

    /// Change selected source, auto-filling the name unless the user has typed one.
    pub fn set_source(&mut self, idx: usize) {
        self.source_idx = idx;
        if !self.name_edited {
            self.name = if idx == 0 {
                String::new()
            } else {
                Self::sources()[idx].to_string()
            };
        }
    }

    pub fn symbols(&self) -> Vec<String> {
        match self.source_idx {
            1 => Index::Sp500.symbols(),
            2 => Index::Nasdaq100.symbols(),
            3 => Index::Dow30.symbols(),
            _ => Vec::new(),
        }
    }
}

#[derive(PartialEq)]
pub enum DeleteTarget {
    Symbol,
    List,
}

pub enum BlacklistFocus {
    View,
    Add(String),
    ConfirmDelete,
}

pub struct BlacklistState {
    pub selected: Option<usize>,
    pub focus: BlacklistFocus,
}

impl BlacklistState {
    pub fn new(len: usize) -> Self {
        Self {
            selected: if len == 0 { None } else { Some(0) },
            focus: BlacklistFocus::View,
        }
    }
}

pub enum Mode {
    Normal,
    AddSymbol(String),
    NewList(NewListState),
    ConfirmDelete(DeleteTarget),
    Detail, // detail popup for selected row
    Blacklist(BlacklistState),
}

// ── Index lookup screen ─────────────────────────────────────────────────────────

/// Result of an async registry+holdings lookup, sent back over a channel.
pub enum LookupResult {
    Found { entry: RegistryEntry, holdings: CachedHoldings },
    NotFound(String),
    Error(String),
}

pub enum IndexLookupPhase {
    Input,
    Loading,
    Loaded { entry: RegistryEntry, holdings: CachedHoldings, table_state: TableState },
    NotFound(String),
    Error(String),
}

pub struct IndexLookupState {
    pub query: String,
    pub phase: IndexLookupPhase,
}

impl IndexLookupState {
    pub fn new() -> Self {
        Self { query: String::new(), phase: IndexLookupPhase::Input }
    }
}

/// Result of an async accounts fetch, sent back over a channel.
pub enum AccountsResult {
    Loaded(Vec<Account>),
    Error(String),
}

pub enum AccountSelectPhase {
    Loading,
    Loaded { accounts: Vec<Account>, table_state: TableState },
    Error(String),
}

pub struct AccountSelectState {
    pub phase: AccountSelectPhase,
}

impl AccountSelectState {
    pub fn new() -> Self {
        Self { phase: AccountSelectPhase::Loading }
    }
}

// ── Amount entry + plan review screens ──────────────────────────────────────────

pub struct AmountEntryState {
    pub entry: RegistryEntry,
    pub holdings: CachedHoldings,
    pub amount_input: String,
    pub error: Option<String>,
}

impl AmountEntryState {
    pub fn new(entry: RegistryEntry, holdings: CachedHoldings) -> Self {
        Self { entry, holdings, amount_input: String::new(), error: None }
    }
}

/// Request sent to the plan-building task: everything it needs, since the
/// task itself is stateless between requests.
pub struct PlanRequest {
    pub index_name: String,
    pub account_hash: String,
    pub holdings: CachedHoldings,
    pub blacklist: Vec<String>,
    pub total_amount: f64,
}

/// Result of an async plan-build, sent back over a channel.
pub enum PlanBuildResult {
    Ready(RebalancePlan),
    Error(String),
}

pub enum PlanReviewPhase {
    Loading,
    Loaded { plan: RebalancePlan, table_state: TableState },
    Error(String),
}

pub struct PlanReviewState {
    pub phase: PlanReviewPhase,
}

impl PlanReviewState {
    pub fn new() -> Self {
        Self { phase: PlanReviewPhase::Loading }
    }
}

/// Request sent to the order-submission task.
pub struct SubmitRequest {
    pub plan: RebalancePlan,
    /// Always `true` from every current call site — live submission is
    /// implemented in orders.rs but deliberately not wired to any TUI
    /// keybinding yet, pending an explicit go-ahead to test it against a
    /// real (tiny) trade before it's trusted for a full plan.
    pub dry_run: bool,
}

pub enum SubmitPhase {
    Submitting,
    Done { results: Vec<OrderResult>, table_state: TableState, viewing: Option<usize> },
}

pub struct SubmitState {
    pub phase: SubmitPhase,
}

impl SubmitState {
    pub fn new() -> Self {
        Self { phase: SubmitPhase::Submitting }
    }
}

// ── Price grid screens ───────────────────────────────────────────────────────────

#[derive(PartialEq)]
pub enum PriceInputFocus {
    Symbol,
    Expiry,
    Iv,
    OptionType,
    Rate,
    DividendYield,
}

pub struct PriceInputState {
    pub symbol: String,
    pub expiry: String,
    pub iv: String,
    pub option_type_idx: usize, // 0 = Call, 1 = Put
    pub rate: String,
    pub dividend_yield: String,
    pub focus: PriceInputFocus,
    pub error: Option<String>,
}

impl PriceInputState {
    pub fn new() -> Self {
        Self {
            symbol: String::new(),
            expiry: String::new(),
            iv: String::new(),
            option_type_idx: 0,
            rate: "0.045".to_string(),
            dividend_yield: "0.0".to_string(),
            focus: PriceInputFocus::Symbol,
            error: None,
        }
    }

    pub fn option_type(&self) -> OptionType {
        if self.option_type_idx == 0 { OptionType::Call } else { OptionType::Put }
    }

    /// Validate all fields and build the request, or return a user-facing
    /// error string — the same validation `pricing::build_price_grid` would
    /// otherwise do, done here first so it surfaces before the async round-trip.
    pub fn build_request(&self) -> Result<PriceRequest, String> {
        let symbol = self.symbol.trim().to_uppercase();
        if symbol.is_empty() {
            return Err("Symbol is required.".to_string());
        }

        let expiry = NaiveDate::parse_from_str(self.expiry.trim(), "%Y-%m-%d")
            .map_err(|_| "Expiry must be a valid date, YYYY-MM-DD.".to_string())?;
        if expiry <= chrono::Utc::now().date_naive() {
            return Err("Expiry must be in the future.".to_string());
        }

        let base_iv = self.iv.trim().parse::<f64>().map_err(|_| "IV must be a number, e.g. 0.30.".to_string())?;
        if base_iv <= 0.0 {
            return Err("IV must be greater than 0.".to_string());
        }

        let rate = self.rate.trim().parse::<f64>().map_err(|_| "Rate must be a number, e.g. 0.045.".to_string())?;
        let dividend_yield = self
            .dividend_yield
            .trim()
            .parse::<f64>()
            .map_err(|_| "Dividend yield must be a number, e.g. 0.0.".to_string())?;

        Ok(PriceRequest { symbol, base_iv, expiry, rate, dividend_yield, option_type: self.option_type() })
    }
}

/// Request sent to the price-grid-building task.
pub struct PriceRequest {
    pub symbol: String,
    pub base_iv: f64,
    pub expiry: NaiveDate,
    pub rate: f64,
    pub dividend_yield: f64,
    pub option_type: OptionType,
}

/// Result of an async grid build, sent back over a channel.
pub enum PriceGridResult {
    Ready(PriceGrid),
    Error(String),
}

pub enum PriceGridPhase {
    Loading,
    Loaded(PriceGrid),
    Error(String),
}

pub struct PriceGridState {
    pub phase: PriceGridPhase,
}

impl PriceGridState {
    pub fn new() -> Self {
        Self { phase: PriceGridPhase::Loading }
    }
}

/// Top-level screen. `Watchlists` is today's existing behavior (driven by
/// `Mode`); other variants are full-screen flows layered on top of it.
pub enum AppScreen {
    Watchlists,
    IndexLookup(IndexLookupState),
    AccountSelect(AccountSelectState),
    AmountEntry(AmountEntryState),
    PlanReview(PlanReviewState),
    Submitting(SubmitState),
    PriceInput(PriceInputState),
    PriceGrid(PriceGridState),
}

// ── App ────────────────────────────────────────────────────────────────────────

pub struct App {
    pub watchlists: Vec<Watchlist>,
    pub active_tab: usize,
    pub table_state: TableState,
    pub quotes: HashMap<String, Quote>,
    pub mode: Mode,
    pub ws_status: Arc<Mutex<String>>,
    pub cmd_tx: mpsc::Sender<StreamCommand>,
    pub enrich_tx: mpsc::Sender<Vec<String>>,
    pub should_quit: bool,
    pub blacklist: Vec<String>,
    pub screen: AppScreen,
    pub lookup_tx: mpsc::Sender<String>,
    pub accounts_tx: mpsc::Sender<()>,
    pub selected_account: Option<Account>,
    pub plan_tx: mpsc::Sender<PlanRequest>,
    pub submit_tx: mpsc::Sender<SubmitRequest>,
    pub price_tx: mpsc::Sender<PriceRequest>,
}

impl App {
    pub fn new(
        watchlists: Vec<Watchlist>,
        ws_status: Arc<Mutex<String>>,
        cmd_tx: mpsc::Sender<StreamCommand>,
        enrich_tx: mpsc::Sender<Vec<String>>,
        blacklist: Vec<String>,
        lookup_tx: mpsc::Sender<String>,
        accounts_tx: mpsc::Sender<()>,
        plan_tx: mpsc::Sender<PlanRequest>,
        submit_tx: mpsc::Sender<SubmitRequest>,
        price_tx: mpsc::Sender<PriceRequest>,
    ) -> Self {
        let mut table_state = TableState::default();
        if !watchlists.is_empty() && !watchlists[0].symbols.is_empty() {
            table_state.select(Some(0));
        }
        Self {
            watchlists,
            active_tab: 0,
            table_state,
            quotes: HashMap::new(),
            mode: Mode::Normal,
            ws_status,
            cmd_tx,
            enrich_tx,
            should_quit: false,
            blacklist,
            screen: AppScreen::Watchlists,
            lookup_tx,
            accounts_tx,
            selected_account: None,
            plan_tx,
            submit_tx,
            price_tx,
        }
    }

    pub fn current_symbols(&self) -> Vec<String> {
        self.watchlists
            .get(self.active_tab)
            .map(|wl| wl.symbols.clone())
            .unwrap_or_default()
    }

    pub fn apply_update(&mut self, u: QuoteUpdate) {
        let q = self.quotes.entry(u.symbol).or_default();
        if let Some(v) = u.bid         { q.bid         = Some(v); }
        if let Some(v) = u.ask         { q.ask         = Some(v); }
        if let Some(v) = u.last        { q.last        = Some(v); }
        if let Some(v) = u.volume      { q.volume      = Some(v); }
        if let Some(v) = u.high        { q.high        = Some(v); }
        if let Some(v) = u.low         { q.low         = Some(v); }
        if let Some(v) = u.prev_close  { q.prev_close  = Some(v); }
        if let Some(v) = u.description { q.description = Some(v); }
        if let Some(v) = u.open        { q.open        = Some(v); }
        if let Some(v) = u.net_change  { q.net_change  = Some(v); }
        if let Some(v) = u.w52_high    { q.w52_high    = Some(v); }
        if let Some(v) = u.w52_low     { q.w52_low     = Some(v); }
        if let Some(v) = u.pe_ratio    { q.pe_ratio    = Some(v); }
        if let Some(v) = u.div_amount  { q.div_amount  = Some(v); }
        if let Some(v) = u.div_yield   { q.div_yield   = Some(v); }
        if let Some(v) = u.eps         { q.eps         = Some(v); }
        if let Some(v) = u.market_cap  { q.market_cap  = Some(v); }
        q.updated = Some(Instant::now());
    }

    /// Switch to a tab by index, notify streamer of new symbols.
    pub fn switch_tab(&mut self, idx: usize) {
        if idx >= self.watchlists.len() { return; }
        self.active_tab = idx;
        let row = if self.current_symbols().is_empty() { None } else { Some(0) };
        self.table_state.select(row);
        self.push_symbols();
    }

    pub fn nav_up(&mut self) {
        let len = self.current_symbols().len();
        if len == 0 { return; }
        let i = match self.table_state.selected() {
            Some(i) if i > 0 => i - 1,
            _ => len - 1,
        };
        self.table_state.select(Some(i));
    }

    pub fn nav_down(&mut self) {
        let len = self.current_symbols().len();
        if len == 0 { return; }
        let i = self.table_state.selected().map(|i| (i + 1) % len).unwrap_or(0);
        self.table_state.select(Some(i));
    }

    pub fn page_up(&mut self) {
        let len = self.current_symbols().len();
        if len == 0 { return; }
        let i = self.table_state.selected().unwrap_or(0).saturating_sub(10);
        self.table_state.select(Some(i));
    }

    pub fn page_down(&mut self) {
        let len = self.current_symbols().len();
        if len == 0 { return; }
        let i = (self.table_state.selected().unwrap_or(0) + 10).min(len - 1);
        self.table_state.select(Some(i));
    }

    pub fn selected_symbol(&self) -> Option<&str> {
        let idx = self.table_state.selected()?;
        self.watchlists.get(self.active_tab)?.symbols.get(idx).map(String::as_str)
    }

    pub fn add_symbol(&mut self, sym: String) {
        let sym = sym.trim().to_uppercase();
        if sym.is_empty() { return; }
        if let Some(wl) = self.watchlists.get_mut(self.active_tab) {
            if !wl.symbols.contains(&sym) {
                wl.symbols.push(sym);
                if self.table_state.selected().is_none() {
                    self.table_state.select(Some(0));
                }
                self.push_symbols();
                let _ = crate::watchlist::save(&self.watchlists);
            }
        }
    }

    pub fn delete_selected_symbol(&mut self) {
        let Some(idx) = self.table_state.selected() else { return };
        if let Some(wl) = self.watchlists.get_mut(self.active_tab) {
            if idx < wl.symbols.len() {
                wl.symbols.remove(idx);
                let new_sel = if wl.symbols.is_empty() { None }
                              else { Some(idx.min(wl.symbols.len() - 1)) };
                self.table_state.select(new_sel);
                self.push_symbols();
                let _ = crate::watchlist::save(&self.watchlists);
            }
        }
    }

    pub fn create_list(&mut self, state: NewListState) {
        let name = state.name.trim().to_string();
        if name.is_empty() { return; }
        let mut wl = Watchlist::new(name);
        wl.symbols = state.symbols();
        self.watchlists.push(wl);
        let new_idx = self.watchlists.len() - 1;
        let _ = crate::watchlist::save(&self.watchlists);
        self.switch_tab(new_idx);
    }

    pub fn delete_active_list(&mut self) {
        if self.watchlists.is_empty() { return; }
        self.watchlists.remove(self.active_tab);
        self.active_tab = self.active_tab.saturating_sub(1);
        let _ = crate::watchlist::save(&self.watchlists);
        self.push_symbols();
        let row = if self.current_symbols().is_empty() { None } else { Some(0) };
        self.table_state.select(row);
    }

    /// Send current watchlist symbols to the streamer and trigger REST enrichment.
    pub fn push_symbols(&self) {
        let syms = self.current_symbols();
        let _ = self.cmd_tx.try_send(StreamCommand::SetSymbols(syms.clone()));
        let _ = self.enrich_tx.try_send(syms);
    }

    pub fn ws_status_text(&self) -> String {
        self.ws_status.lock().map(|s| s.clone()).unwrap_or_default()
    }

    pub fn add_blacklist_symbol(&mut self, sym: String) {
        let sym = sym.trim().to_uppercase();
        if sym.is_empty() { return; }
        if !self.blacklist.contains(&sym) {
            self.blacklist.push(sym);
            let _ = crate::blacklist::save(&self.blacklist);
        }
    }

    /// Remove the blacklist entry at `idx`, returning the new selection index.
    pub fn delete_blacklist_symbol(&mut self, idx: usize) -> Option<usize> {
        if idx >= self.blacklist.len() { return None; }
        self.blacklist.remove(idx);
        let _ = crate::blacklist::save(&self.blacklist);
        if self.blacklist.is_empty() { None } else { Some(idx.min(self.blacklist.len() - 1)) }
    }

    /// Apply an async lookup result. Guarded so a late-arriving result from a
    /// query the user has already navigated away from is silently dropped.
    pub fn apply_lookup_result(&mut self, result: LookupResult) {
        let AppScreen::IndexLookup(ref mut state) = self.screen else { return };
        state.phase = match result {
            LookupResult::Found { entry, holdings } => {
                let mut table_state = TableState::default();
                if !holdings.constituents.is_empty() { table_state.select(Some(0)); }
                IndexLookupPhase::Loaded { entry, holdings, table_state }
            }
            LookupResult::NotFound(q) => IndexLookupPhase::NotFound(q),
            LookupResult::Error(e) => IndexLookupPhase::Error(e),
        };
    }

    pub fn nav_lookup_down(&mut self) {
        let AppScreen::IndexLookup(ref mut state) = self.screen else { return };
        let IndexLookupPhase::Loaded { ref holdings, ref mut table_state, .. } = state.phase else { return };
        let len = holdings.constituents.len();
        if len == 0 { return; }
        let i = table_state.selected().map(|i| (i + 1) % len).unwrap_or(0);
        table_state.select(Some(i));
    }

    pub fn nav_lookup_up(&mut self) {
        let AppScreen::IndexLookup(ref mut state) = self.screen else { return };
        let IndexLookupPhase::Loaded { ref holdings, ref mut table_state, .. } = state.phase else { return };
        let len = holdings.constituents.len();
        if len == 0 { return; }
        let i = match table_state.selected() {
            Some(i) if i > 0 => i - 1,
            _ => len - 1,
        };
        table_state.select(Some(i));
    }

    pub fn apply_accounts_result(&mut self, result: AccountsResult) {
        let AppScreen::AccountSelect(ref mut state) = self.screen else { return };
        state.phase = match result {
            AccountsResult::Loaded(accounts) => {
                let mut table_state = TableState::default();
                if !accounts.is_empty() { table_state.select(Some(0)); }
                AccountSelectPhase::Loaded { accounts, table_state }
            }
            AccountsResult::Error(e) => AccountSelectPhase::Error(e),
        };
    }

    pub fn nav_account_down(&mut self) {
        let AppScreen::AccountSelect(ref mut state) = self.screen else { return };
        let AccountSelectPhase::Loaded { ref accounts, ref mut table_state } = state.phase else { return };
        let len = accounts.len();
        if len == 0 { return; }
        let i = table_state.selected().map(|i| (i + 1) % len).unwrap_or(0);
        table_state.select(Some(i));
    }

    pub fn nav_account_up(&mut self) {
        let AppScreen::AccountSelect(ref mut state) = self.screen else { return };
        let AccountSelectPhase::Loaded { ref accounts, ref mut table_state } = state.phase else { return };
        let len = accounts.len();
        if len == 0 { return; }
        let i = match table_state.selected() {
            Some(i) if i > 0 => i - 1,
            _ => len - 1,
        };
        table_state.select(Some(i));
    }

    /// Confirm the highlighted account: persist its hash to Config and hold
    /// the resolved Account in memory, then return to the Watchlists screen.
    pub fn confirm_account_selection(&mut self) {
        let AppScreen::AccountSelect(ref state) = self.screen else { return };
        let AccountSelectPhase::Loaded { ref accounts, ref table_state } = state.phase else { return };
        let Some(idx) = table_state.selected() else { return };
        let Some(account) = accounts.get(idx) else { return };

        if let Ok(mut cfg) = crate::config::Config::load() {
            cfg.selected_account_hash = Some(account.hash_value.clone());
            let _ = cfg.save();
        }
        self.selected_account = Some(account.clone());
        self.screen = AppScreen::Watchlists;
    }

    pub fn apply_plan_result(&mut self, result: PlanBuildResult) {
        let AppScreen::PlanReview(ref mut state) = self.screen else { return };
        state.phase = match result {
            PlanBuildResult::Ready(plan) => {
                let mut table_state = TableState::default();
                if !plan.lines.is_empty() { table_state.select(Some(0)); }
                PlanReviewPhase::Loaded { plan, table_state }
            }
            PlanBuildResult::Error(e) => PlanReviewPhase::Error(e),
        };
    }

    pub fn nav_plan_down(&mut self) {
        let AppScreen::PlanReview(ref mut state) = self.screen else { return };
        let PlanReviewPhase::Loaded { ref plan, ref mut table_state } = state.phase else { return };
        let len = plan.lines.len();
        if len == 0 { return; }
        let i = table_state.selected().map(|i| (i + 1) % len).unwrap_or(0);
        table_state.select(Some(i));
    }

    pub fn nav_plan_up(&mut self) {
        let AppScreen::PlanReview(ref mut state) = self.screen else { return };
        let PlanReviewPhase::Loaded { ref plan, ref mut table_state } = state.phase else { return };
        let len = plan.lines.len();
        if len == 0 { return; }
        let i = match table_state.selected() {
            Some(i) if i > 0 => i - 1,
            _ => len - 1,
        };
        table_state.select(Some(i));
    }

    pub fn apply_submit_result(&mut self, results: Vec<OrderResult>) {
        let AppScreen::Submitting(ref mut state) = self.screen else { return };
        let mut table_state = TableState::default();
        if !results.is_empty() { table_state.select(Some(0)); }
        state.phase = SubmitPhase::Done { results, table_state, viewing: None };
    }

    pub fn nav_submit_down(&mut self) {
        let AppScreen::Submitting(ref mut state) = self.screen else { return };
        let SubmitPhase::Done { ref results, ref mut table_state, .. } = state.phase else { return };
        let len = results.len();
        if len == 0 { return; }
        let i = table_state.selected().map(|i| (i + 1) % len).unwrap_or(0);
        table_state.select(Some(i));
    }

    pub fn nav_submit_up(&mut self) {
        let AppScreen::Submitting(ref mut state) = self.screen else { return };
        let SubmitPhase::Done { ref results, ref mut table_state, .. } = state.phase else { return };
        let len = results.len();
        if len == 0 { return; }
        let i = match table_state.selected() {
            Some(i) if i > 0 => i - 1,
            _ => len - 1,
        };
        table_state.select(Some(i));
    }

    /// Toggle the detail popup for the highlighted row (view the raw request
    /// JSON that was built for it).
    pub fn toggle_submit_detail(&mut self) {
        let AppScreen::Submitting(ref mut state) = self.screen else { return };
        let SubmitPhase::Done { ref table_state, ref mut viewing, .. } = state.phase else { return };
        *viewing = if viewing.is_some() { None } else { table_state.selected() };
    }

    pub fn apply_price_grid_result(&mut self, result: PriceGridResult) {
        let AppScreen::PriceGrid(ref mut state) = self.screen else { return };
        state.phase = match result {
            PriceGridResult::Ready(grid) => PriceGridPhase::Loaded(grid),
            PriceGridResult::Error(e) => PriceGridPhase::Error(e),
        };
    }
}
