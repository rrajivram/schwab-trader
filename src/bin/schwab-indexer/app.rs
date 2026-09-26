use std::collections::{BTreeSet, HashMap};

use eframe::egui;
use schwab::{
    accounts::Account,
    api::MarketData,
    blacklist,
    config::Config,
    portfolio::{held_positions, Held},
    universe::Universe,
};

use crate::home::SortState;
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
    worker: Worker,
    screen: Screen,
    pub(crate) account: Remote<Account>,
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
    pub(crate) mode: Mode,
    /// Create mode: chosen symbols, in the order added.
    pub(crate) basket: Vec<String>,
    pub(crate) auto_size: usize,
    /// Rebalance mode: held symbols to sell, in the order ticked.
    pub(crate) discards: Vec<String>,
    pub(crate) review: Option<Review>,
}

impl IndexerApp {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        let worker = Worker::new(cc.egui_ctx.clone());
        worker.check_token();
        let (dnt, dnt_error) = match blacklist::load() {
            Ok(v) => (v.into_iter().collect(), None),
            Err(e) => (BTreeSet::new(), Some(format!("Could not read do-not-transact list: {e}"))),
        };
        Self {
            worker,
            screen: Screen::Starting,
            account: Remote::default(),
            universe: Remote::default(),
            market: Remote::default(),
            dividends: Remote::default(),
            held: HashMap::new(),
            dnt,
            dnt_input: String::new(),
            dnt_error,
            show_dnt_panel: false,
            sort: HashMap::new(),
            mode: Mode::Browse,
            basket: Vec::new(),
            auto_size: 20,
            discards: Vec::new(),
            review: None,
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
        self.worker.load_account();
    }

    pub(crate) fn load_universe(&mut self, force: bool) {
        self.universe.start();
        self.worker.load_universe(force);
    }

    pub(crate) fn load_market(&mut self) {
        let Some(u) = &self.universe.value else { return };
        let symbols = u.constituents.iter().map(|c| c.symbol.clone()).collect();
        self.market.start();
        self.worker.load_market(symbols);
    }

    /// Company names (Schwab's description) for the given tickers; tickers
    /// without a quote yet get an empty name. Collected up front so panels can
    /// show names while mutably borrowing other fields.
    pub(crate) fn names<'a>(&self, symbols: impl IntoIterator<Item = &'a String>) -> HashMap<String, String> {
        let market = self.market.value.as_ref();
        symbols
            .into_iter()
            .map(|s| {
                let name = market.and_then(|m| m.get(s)?.description.clone()).unwrap_or_default();
                (s.clone(), name)
            })
            .collect()
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
            Msg::AccountLoaded(result) => {
                self.account.finish(result);
                self.rebuild_held();
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
            Msg::MarketLoaded(result) => self.market.finish(result),
        }
    }

    fn login_ui(&mut self, ui: &mut egui::Ui) {
        let Screen::Login(form) = &mut self.screen else { return };

        ui.vertical_centered(|ui| {
            ui.add_space(60.0);
            ui.heading("Connect to Schwab");
            ui.add_space(20.0);
        });

        egui::Grid::new("login_grid").num_columns(2).spacing([12.0, 10.0]).show(ui, |ui| {
            ui.label("App key");
            ui.add(egui::TextEdit::singleline(&mut form.app_key).desired_width(420.0));
            ui.end_row();
            ui.label("App secret");
            ui.add(egui::TextEdit::singleline(&mut form.app_secret).password(true).desired_width(420.0));
            ui.end_row();
        });

        ui.add_space(10.0);
        let can_begin = !form.busy && !form.app_key.trim().is_empty() && !form.app_secret.trim().is_empty();
        if ui.add_enabled(can_begin, egui::Button::new("1. Open Schwab login in browser")).clicked() {
            form.busy = true;
            form.error = None;
            self.worker.begin_login(form.app_key.trim().to_string(), form.app_secret.trim().to_string());
        }

        if let Some(url) = &form.auth_url {
            ui.add_space(10.0);
            ui.label("If the browser didn't open, visit:");
            ui.hyperlink(url);
            ui.add_space(10.0);
            ui.label("After approving, the browser redirects to a page that fails to load. \
                      Copy the full URL from the address bar and paste it here:");
            ui.add(egui::TextEdit::singleline(&mut form.pasted_url).desired_width(f32::INFINITY));
            let can_complete = !form.busy && !form.pasted_url.trim().is_empty();
            if ui.add_enabled(can_complete, egui::Button::new("2. Complete login")).clicked() {
                form.busy = true;
                form.error = None;
                self.worker.complete_login(form.pasted_url.trim().to_string());
            }
        }

        if form.busy {
            ui.spinner();
        }
        if let Some(err) = &form.error {
            ui.add_space(10.0);
            ui.colored_label(ui.visuals().error_fg_color, err);
        }
    }

    pub(crate) fn relogin(&mut self) {
        self.screen = Screen::Login(LoginForm::prefilled(None));
    }
}

impl eframe::App for IndexerApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
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
                egui::CentralPanel::default().show(ui, |ui| self.login_ui(ui));
            }
            Screen::Home => self.home_ui(ui),
        }
    }
}

/// Ticker in monospace followed by the company name, dimmed and truncated.
pub fn ticker_with_name(ui: &mut egui::Ui, symbol: &str, name: &str) {
    ui.monospace(symbol);
    if !name.is_empty() {
        ui.add(egui::Label::new(egui::RichText::new(name).weak()).truncate()).on_hover_text(name);
    }
}

/// "F (FORD MTR CO DEL)", or just the ticker when the name is unknown.
pub fn ticker_and_name(symbol: &str, name: &str) -> String {
    if name.is_empty() { symbol.to_string() } else { format!("{symbol} ({name})") }
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
