use std::collections::{BTreeSet, HashMap};

use eframe::egui::{self, RichText};
use schwab::{
    accounts::Account,
    api::MarketData,
    blacklist,
    config::Config,
    portfolio::{held_positions, Held},
    universe::Universe,
};

use crate::execute::Execution;
use crate::home::SortState;
use crate::theme;
use crate::modes::{Mode, Review};
use crate::worker::{Msg, Worker};

enum Screen {
    /// Checking stored tokens on startup.
    Starting,
    Login(LoginForm),
    Home,
}

#[derive(Default)]
struct LoginForm {
    app_key: String,
    app_secret: String,
    /// Set once the authorize URL has been generated and the browser opened.
    auth_url: Option<String>,
    pasted_url: String,
    busy: bool,
    error: Option<String>,
}

impl LoginForm {
    fn prefilled(error: Option<String>) -> Self {
        let cfg = Config::load().unwrap_or_default();
        Self {
            app_key: cfg.app_key.unwrap_or_default(),
            app_secret: cfg.app_secret.unwrap_or_default(),
            error,
            ..Default::default()
        }
    }
}

/// A value fetched in the background, with its in-flight/error state.
pub struct Remote<T> {
    pub value: Option<T>,
    pub loading: bool,
    pub error: Option<String>,
}

impl<T> Default for Remote<T> {
    fn default() -> Self {
        Self { value: None, loading: false, error: None }
    }
}

impl<T> Remote<T> {
    fn start(&mut self) {
        self.loading = true;
        self.error = None;
    }

    fn finish(&mut self, result: Result<T, String>) {
        self.loading = false;
        match result {
            Ok(v) => self.value = Some(v),
            Err(e) => self.error = Some(e),
        }
    }
}

pub struct IndexerApp {
    pub(crate) worker: Worker,
    screen: Screen,
    /// The equities account (last digits from config, default 343): Home,
    /// Create, Rebalance, dividends and every order use only this one.
    pub(crate) account: Remote<Account>,
    /// Every linked account, for the Bonds tab.
    pub(crate) accounts: Remote<Vec<Account>>,
    pub(crate) universe: Remote<Universe>,
    pub(crate) market: Remote<HashMap<String, MarketData>>,
    pub(crate) dividends: Remote<HashMap<String, f64>>,
    /// Long positions keyed by universe symbol; rebuilt when the account or
    /// dividends change.
    pub(crate) held: HashMap<String, Held>,
    /// "Do not transact" symbols — the same blacklist.json the CLI uses.
    pub(crate) dnt: BTreeSet<String>,
    pub(crate) dnt_input: String,
    pub(crate) dnt_error: Option<String>,
    pub(crate) show_dnt_panel: bool,
    pub(crate) sort: HashMap<String, SortState>,
    /// Home list search (ticker or name).
    pub(crate) search: String,
    pub(crate) mode: Mode,
    /// Create mode: chosen symbols, in the order added.
    pub(crate) basket: Vec<String>,
    pub(crate) auto_size: usize,
    /// Auto fill only picks stocks with 0 < P/E ≤ `max_pe` when enabled.
    pub(crate) max_pe_enabled: bool,
    pub(crate) max_pe: f64,
    /// Result message from the last Auto fill (e.g. fewer stocks than asked).
    pub(crate) auto_note: Option<String>,
    /// Rebalance mode: held symbols to sell, in the order ticked.
    pub(crate) discards: Vec<String>,
    pub(crate) review: Option<Review>,
    pub(crate) exec: Option<Execution>,
    /// Alpha Vantage overviews by symbol (disk cache + this session's fetches).
    pub(crate) overviews: HashMap<String, schwab::alphavantage::Overview>,
    pub(crate) av_running: bool,
    /// Symbols requested while a run was in progress; fetched next.
    pub(crate) av_pending: Vec<String>,
    pub(crate) av_notes: Vec<String>,
    pub(crate) av_used_today: u32,
    /// Weekly closes by symbol (disk cache + this session's fetches).
    pub(crate) history: HashMap<String, schwab::history::Series>,
    pub(crate) risk_free: Remote<f64>,
    /// Alpha/beta fits against the benchmark, refreshed as history or the
    /// rate arrives.
    pub(crate) fits: HashMap<String, schwab::risk::Fit>,
    pub(crate) history_running: bool,
    /// Symbols requested while a run was in progress; fetched next.
    pub(crate) history_pending: Vec<String>,
    /// (fetched, requested) in the current run.
    pub(crate) history_progress: (usize, usize),
    pub(crate) history_failed: Vec<String>,
    /// Factor weights (saved in config) and the scores they produce.
    pub(crate) weights: schwab::factors::Weights,
    pub(crate) scores: HashMap<String, schwab::factors::Score>,
    pub(crate) settings: Option<SettingsForm>,
    /// Bonds tab sort: (column, ascending); None = by maturity.
    pub(crate) bond_sort: Option<(usize, bool)>,
    pub(crate) plan: crate::plan_tab::PlanView,
    pub(crate) backtest: crate::backtest_tab::BacktestView,
}

pub struct SettingsForm {
    pub plan_page: String,
    pub av_key: String,
    pub saved: bool,
}

impl IndexerApp {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        let worker = Worker::new(cc.egui_ctx.clone());
        worker.check_token();
        let av_cache = schwab::alphavantage::Cache::load();
        let (dnt, dnt_error) = match blacklist::load() {
            Ok(v) => (v.into_iter().collect(), None),
            Err(e) => (BTreeSet::new(), Some(format!("Could not read do-not-transact list: {e}"))),
        };
        Self {
            worker,
            screen: Screen::Starting,
            account: Remote::default(),
            accounts: Remote::default(),
            universe: Remote::default(),
            market: Remote::default(),
            dividends: Remote::default(),
            held: HashMap::new(),
            dnt,
            dnt_input: String::new(),
            dnt_error,
            show_dnt_panel: false,
            sort: HashMap::new(),
            search: String::new(),
            // INDEXER_TAB=bonds|plan opens straight onto that tab.
            mode: match std::env::var("INDEXER_TAB").as_deref() {
                Ok("bonds") => Mode::Bonds,
                Ok("plan") => Mode::Plan,
                Ok("backtest") => Mode::Backtest,
                _ => Mode::Browse,
            },
            basket: Vec::new(),
            auto_size: 20,
            max_pe_enabled: false,
            max_pe: 25.0,
            auto_note: None,
            discards: Vec::new(),
            review: None,
            exec: None,
            overviews: av_cache.overviews.clone(),
            av_running: false,
            av_pending: Vec::new(),
            av_notes: Vec::new(),
            av_used_today: av_cache.used_today(),
            history: schwab::history::Cache::load().series,
            risk_free: Remote::default(),
            fits: HashMap::new(),
            history_running: false,
            history_pending: Vec::new(),
            history_progress: (0, 0),
            history_failed: Vec::new(),
            weights: Config::load().ok().and_then(|c| c.factor_weights).unwrap_or_default(),
            scores: HashMap::new(),
            settings: None,
            bond_sort: None,
            plan: Default::default(),
            backtest: Default::default(),
        }
    }

    fn enter_home(&mut self) {
        self.screen = Screen::Home;
        self.load_account();
        if self.universe.value.is_none() {
            self.load_universe(false);
        } else {
            self.load_market();
        }
    }

    pub(crate) fn load_account(&mut self) {
        self.account.start();
        self.accounts.start();
        self.worker.load_accounts();
    }

    pub(crate) fn load_universe(&mut self, force: bool) {
        self.universe.start();
        self.worker.load_universe(force);
    }

    pub(crate) fn load_market(&mut self) {
        let Some(u) = &self.universe.value else { return };
        // The index, plus anything held outside it (ETFs, other stocks).
        let mut symbols: Vec<String> = u.constituents.iter().map(|c| c.symbol.clone()).collect();
        for s in self.held.keys() {
            if !symbols.contains(s) {
                symbols.push(s.clone());
            }
        }
        self.market.start();
        self.worker.load_market(symbols);
    }

    /// Company name: Alpha Vantage's readable one when cached, otherwise
    /// Schwab's description, otherwise empty.
    pub(crate) fn display_name(&self, symbol: &str) -> String {
        self.overviews
            .get(symbol)
            .and_then(|o| o.name.clone())
            .or_else(|| self.market.value.as_ref()?.get(symbol)?.description.clone())
            .unwrap_or_default()
    }

    /// Names for the given tickers, collected up front so panels can show
    /// them while mutably borrowing other fields.
    pub(crate) fn names<'a>(&self, symbols: impl IntoIterator<Item = &'a String>) -> HashMap<String, String> {
        symbols.into_iter().map(|s| (s.clone(), self.display_name(s))).collect()
    }

    /// Fetch weekly price history (benchmark first) for symbols without fresh
    /// cached data, plus the risk-free rate once per session.
    pub(crate) fn fetch_history(&mut self, symbols: Vec<String>) {
        use schwab::history::BENCHMARK;
        let mut needed: Vec<String> = std::iter::once(BENCHMARK.to_string())
            .chain(symbols)
            .filter(|s| !self.history.get(s).is_some_and(|h| h.is_fresh()))
            .collect();
        needed.sort();
        needed.dedup();
        if let Some(i) = needed.iter().position(|s| s == BENCHMARK) {
            needed.swap(0, i);
        }
        if self.history_running {
            self.history_pending.extend(needed);
            return;
        }
        let need_rate = self.risk_free.value.is_none() && !self.risk_free.loading;
        if needed.is_empty() && !need_rate {
            return;
        }
        if need_rate {
            self.risk_free.start();
        }
        self.history_running = true;
        self.history_progress = (0, needed.len());
        self.worker.load_history(needed, need_rate);
    }

    fn refit(&mut self, symbol: &str) {
        let (Some(&rf), Some(market)) = (self.risk_free.value.as_ref(), self.history.get(schwab::history::BENCHMARK)) else { return };
        // The table's alpha covers the latest 3 years; the cache holds 10 for backtests.
        let market = schwab::history::tail(&market.closes, schwab::history::FIT_WEEKS);
        match self.history.get(symbol).and_then(|s| schwab::risk::fit(&s.closes, market, rf)) {
            Some(f) => self.fits.insert(symbol.to_string(), f),
            None => self.fits.remove(symbol),
        };
    }

    fn refit_all(&mut self) {
        let symbols: Vec<String> = self.history.keys().cloned().collect();
        for s in symbols {
            self.refit(&s);
        }
    }

    /// Recompute factor scores for the index; cheap enough to run on every
    /// input change (a few sorts over 500 stocks).
    pub(crate) fn rescore(&mut self) {
        let (Some(u), Some(m)) = (&self.universe.value, &self.market.value) else { return };
        let stocks: Vec<_> = u
            .constituents
            .iter()
            .map(|c| {
                let closes = self.history.get(&c.symbol).map(|h| h.closes.as_slice());
                let beta = self.fits.get(&c.symbol).map(|f| f.beta);
                (c.symbol.clone(), c.sector.clone(), schwab::factors::Inputs::new(m.get(&c.symbol), closes, beta))
            })
            .collect();
        self.scores = schwab::factors::score(&stocks, &self.weights);
    }

    pub(crate) fn save_weights(&self) {
        if let Ok(mut c) = Config::load() {
            c.factor_weights = Some(self.weights);
            let _ = c.save();
        }
    }

    /// Queue Alpha Vantage lookups for symbols without fresh cached data.
    pub(crate) fn fetch_overviews(&mut self, symbols: Vec<String>) {
        let needed: Vec<String> = symbols
            .into_iter()
            .filter(|s| !self.overviews.get(s).is_some_and(|o| o.is_fresh()))
            .collect();
        if needed.is_empty() {
            return;
        }
        if self.av_running {
            self.av_pending.extend(needed);
            return;
        }
        let Some(key) = Config::load().ok().and_then(|c| c.alphavantage_key).filter(|k| !k.trim().is_empty()) else {
            self.av_notes = vec!["Add an Alpha Vantage key in Settings to load forward P/E and analyst data.".into()];
            return;
        };
        self.av_notes.clear();
        self.av_running = true;
        self.worker.load_overviews(key, needed);
    }

    fn rebuild_held(&mut self) {
        let empty = HashMap::new();
        let divs = self.dividends.value.as_ref().unwrap_or(&empty);
        self.held = match &self.account.value {
            Some(acct) => held_positions(acct, divs),
            None => HashMap::new(),
        };
    }

    pub(crate) fn set_dnt(&mut self, symbol: &str, on: bool) {
        let changed = if on { self.dnt.insert(symbol.to_string()) } else { self.dnt.remove(symbol) };
        if on {
            self.basket.retain(|s| s != symbol);
            self.discards.retain(|s| s != symbol);
        }
        if changed {
            let list: Vec<String> = self.dnt.iter().cloned().collect();
            self.dnt_error = blacklist::save(&list).err().map(|e| format!("Could not save list: {e}"));
        }
    }

    fn handle(&mut self, msg: Msg) {
        match msg {
            Msg::TokenChecked(Ok(())) => self.enter_home(),
            Msg::TokenChecked(Err(_)) => {
                // Missing or unrefreshable tokens both just mean "log in".
                self.screen = Screen::Login(LoginForm::prefilled(None));
            }
            Msg::LoginBegun(result) => {
                if let Screen::Login(form) = &mut self.screen {
                    form.busy = false;
                    match result {
                        Ok(url) => {
                            let _ = webbrowser::open(&url);
                            form.auth_url = Some(url);
                        }
                        Err(e) => form.error = Some(e),
                    }
                }
            }
            Msg::LoginCompleted(Ok(())) => self.enter_home(),
            Msg::LoginCompleted(Err(e)) => {
                if let Screen::Login(form) = &mut self.screen {
                    form.busy = false;
                    form.error = Some(e);
                }
            }
            Msg::AccountsLoaded(result) => {
                let suffix = Config::load().map(|c| c.equities_account()).unwrap_or_else(|_| schwab::config::DEFAULT_EQUITIES_ACCOUNT.to_string());
                let equities = result.clone().and_then(|all| schwab::accounts::by_suffix(&all, &suffix).cloned());
                self.accounts.finish(result);
                self.account.finish(equities);
                self.rebuild_held();
                // New holdings outside the index need quotes too.
                let missing = self.market.value.as_ref().is_some_and(|m| self.held.keys().any(|s| !m.contains_key(s)));
                if missing && !self.market.loading {
                    self.load_market();
                }
                if let Some(acct) = &self.account.value {
                    self.dividends.start();
                    let held: Vec<String> = self.held.keys().cloned().collect();
                    self.worker.load_dividends(acct.hash_value.clone(), held);
                }
            }
            Msg::DividendsLoaded(result) => {
                self.dividends.finish(result);
                self.rebuild_held();
            }
            Msg::UniverseLoaded(result) => {
                self.universe.finish(result);
                self.load_market();
            }
            Msg::MarketLoaded(result) => {
                self.market.finish(result);
                self.rescore();
                if let Some(m) = &self.market.value {
                    let symbols = m.keys().cloned().collect();
                    self.fetch_history(symbols);
                }
            }
            Msg::RiskFree(result) => {
                self.risk_free.finish(result);
                self.refit_all();
                self.rescore();
            }
            Msg::History(sym, series) => {
                self.history.insert(sym.clone(), series);
                self.history_progress.0 += 1;
                if sym == schwab::history::BENCHMARK {
                    self.refit_all();
                } else {
                    self.refit(&sym);
                }
                self.rescore();
            }
            Msg::Backtest(params, outcome) => self.finish_backtest(params, outcome.map(|o| *o)),
            Msg::HistoryDone(failed) => {
                self.history_running = false;
                self.history_failed = failed;
                let pending = std::mem::take(&mut self.history_pending);
                self.fetch_history(pending);
            }
            Msg::Exec(ev) => self.handle_exec(ev),
            Msg::Overview(sym, o) => {
                self.overviews.insert(sym, o);
            }
            Msg::OverviewNote(n) => self.av_notes.push(n),
            Msg::OverviewsDone(used) => {
                self.av_running = false;
                self.av_used_today = used;
                let pending = std::mem::take(&mut self.av_pending);
                self.fetch_overviews(pending);
            }
        }
    }

    fn login_ui(&mut self, ui: &mut egui::Ui) {
        let p = crate::theme::pal(ui);
        let Screen::Login(form) = &mut self.screen else { return };

        ui.vertical_centered(|ui| {
            ui.add_space((ui.available_height() * 0.14).max(24.0));
            ui.label(RichText::new("Indexer").font(theme::sans_semibold(30.0)).color(p.ink));
            ui.label(RichText::new("Build and rebalance an S&P 500 basket in your Schwab account").color(p.muted));
            ui.add_space(20.0);
        });

        let width = 460.0;
        ui.vertical_centered(|ui| {
            ui.set_max_width(width);
            theme::card(ui).inner_margin(egui::Margin::same(22)).show(ui, |ui| {
                ui.set_width(width - 44.0);
                ui.label(RichText::new("Connect to Schwab").font(theme::sans_semibold(theme::TITLE)));
                ui.label(RichText::new("Use the app key and secret from your Schwab developer portal. They're saved on this Mac only.").color(p.muted));
                ui.add_space(12.0);

                ui.label(theme::eyebrow(ui, "App key"));
                ui.add(egui::TextEdit::singleline(&mut form.app_key).desired_width(f32::INFINITY));
                ui.add_space(4.0);
                ui.label(theme::eyebrow(ui, "App secret"));
                ui.add(egui::TextEdit::singleline(&mut form.app_secret).password(true).desired_width(f32::INFINITY));
                ui.add_space(12.0);

                let can_begin = !form.busy && !form.app_key.trim().is_empty() && !form.app_secret.trim().is_empty();
                let label = if form.auth_url.is_some() { "Open Schwab login again" } else { "Step 1 · Open Schwab login" };
                if ui.add_enabled_ui(can_begin, |ui| ui.add_sized([ui.available_width(), 34.0], theme::primary(ui, label))).inner.clicked() {
                    form.busy = true;
                    form.error = None;
                    self.worker.begin_login(form.app_key.trim().to_string(), form.app_secret.trim().to_string());
                }

                if let Some(url) = &form.auth_url {
                    ui.add_space(14.0);
                    ui.separator();
                    ui.label(RichText::new("Step 2 · Paste the redirect URL").font(theme::sans_semibold(theme::BODY)));
                    ui.label(
                        RichText::new(
                            "After you approve access, the browser goes to a page that won't load. \
                             That's expected. Copy the whole address from the address bar and paste it here.",
                        )
                        .color(p.muted),
                    );
                    ui.add(egui::TextEdit::singleline(&mut form.pasted_url).hint_text("https://127.0.0.1/?code=…").desired_width(f32::INFINITY));
                    ui.add_space(6.0);
                    let can_complete = !form.busy && !form.pasted_url.trim().is_empty();
                    if ui.add_enabled_ui(can_complete, |ui| ui.add_sized([ui.available_width(), 34.0], theme::primary(ui, "Finish connecting"))).inner.clicked() {
                        form.busy = true;
                        form.error = None;
                        self.worker.complete_login(form.pasted_url.trim().to_string());
                    }
                    ui.add_space(4.0);
                    ui.horizontal_wrapped(|ui| {
                        ui.label(RichText::new("Browser didn't open?").color(p.muted).font(theme::sans(theme::SMALL)));
                        ui.hyperlink_to(RichText::new("Open the login page").font(theme::sans(theme::SMALL)), url);
                    });
                }

                if form.busy {
                    ui.add_space(6.0);
                    ui.spinner();
                }
                if let Some(err) = &form.error {
                    ui.add_space(8.0);
                    ui.label(RichText::new(err).color(p.loss));
                }
            });
        });
    }

    pub(crate) fn relogin(&mut self) {
        self.screen = Screen::Login(LoginForm::prefilled(None));
    }
}

impl eframe::App for IndexerApp {
    fn ui(&mut self, ui: &mut egui::Ui, frame: &mut eframe::Frame) {
        while let Ok(msg) = self.worker.rx.try_recv() {
            self.handle(msg);
        }

        match self.screen {
            Screen::Starting => {
                egui::CentralPanel::default().show(ui, |ui| {
                    ui.centered_and_justified(|ui| ui.spinner());
                });
            }
            Screen::Login(_) => {
                let bg = theme::pal(ui).bg;
                egui::CentralPanel::default().frame(egui::Frame::new().fill(bg).inner_margin(egui::Margin::same(16))).show(ui, |ui| {
                    egui::ScrollArea::vertical().show(ui, |ui| self.login_ui(ui));
                });
            }
            Screen::Home => self.home_ui(ui, frame),
        }
        // The native web view floats above egui, so it must be hidden
        // explicitly whenever the Plan tab isn't the thing on screen.
        if !(matches!(self.screen, Screen::Home) && self.mode == Mode::Plan) {
            self.plan.hide();
        }
    }
}

/// `$1,234,567.89` style formatting.
pub fn money(v: f64) -> String {
    let neg = v < 0.0;
    let cents = format!("{:.2}", v.abs());
    let (int, frac) = cents.split_once('.').unwrap_or((&cents, "00"));
    format!("{}${}.{}", if neg { "-" } else { "" }, group_thousands(int), frac)
}

pub fn group_thousands(int: &str) -> String {
    let mut grouped = String::new();
    for (i, c) in int.chars().enumerate() {
        if i > 0 && (int.len() - i) % 3 == 0 {
            grouped.push(',');
        }
        grouped.push(c);
    }
    grouped
}
