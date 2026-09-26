use eframe::egui;
use schwab::{accounts::Account, config::Config};

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

pub struct IndexerApp {
    worker: Worker,
    screen: Screen,
    account: Option<Account>,
    account_loading: bool,
    account_error: Option<String>,
}

impl IndexerApp {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        let worker = Worker::new(cc.egui_ctx.clone());
        worker.check_token();
        Self {
            worker,
            screen: Screen::Starting,
            account: None,
            account_loading: false,
            account_error: None,
        }
    }

    fn load_account(&mut self) {
        self.account_loading = true;
        self.account_error = None;
        self.worker.load_account();
    }

    fn handle(&mut self, msg: Msg) {
        match msg {
            Msg::TokenChecked(Ok(())) => {
                self.screen = Screen::Home;
                self.load_account();
            }
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
            Msg::LoginCompleted(Ok(())) => {
                self.screen = Screen::Home;
                self.load_account();
            }
            Msg::LoginCompleted(Err(e)) => {
                if let Screen::Login(form) = &mut self.screen {
                    form.busy = false;
                    form.error = Some(e);
                }
            }
            Msg::AccountLoaded(result) => {
                self.account_loading = false;
                match result {
                    Ok(acct) => self.account = Some(acct),
                    Err(e) => self.account_error = Some(e),
                }
            }
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

    fn home_ui(&mut self, ui: &mut egui::Ui) {
        egui::Panel::top("account_bar").show(ui, |ui| {
            ui.horizontal(|ui| {
                match (&self.account, self.account_loading) {
                    (_, true) => {
                        ui.spinner();
                        ui.label("Loading account…");
                    }
                    (Some(acct), false) => {
                        ui.label(format!("Account …{}", last4(&acct.account_number)));
                        ui.separator();
                        ui.strong(format!("Cash: {}", money(acct.cash_balance)));
                    }
                    (None, false) => {
                        ui.label("No account loaded");
                    }
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.button("Log in again").clicked() {
                        self.screen = Screen::Login(LoginForm::prefilled(None));
                        return;
                    }
                    if ui.add_enabled(!self.account_loading, egui::Button::new("Refresh")).clicked() {
                        self.load_account();
                    }
                });
            });
            if let Some(err) = &self.account_error {
                ui.colored_label(ui.visuals().error_fg_color, err);
            }
        });

        egui::CentralPanel::default().show(ui, |ui| {
            ui.label("S&P 500 view coming in the next phase.");
        });
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

fn last4(s: &str) -> &str {
    &s[s.len().saturating_sub(4)..]
}

/// `$1,234,567.89` style formatting.
pub fn money(v: f64) -> String {
    let neg = v < 0.0;
    let cents = format!("{:.2}", v.abs());
    let (int, frac) = cents.split_once('.').unwrap_or((&cents, "00"));
    let mut grouped = String::new();
    for (i, c) in int.chars().enumerate() {
        if i > 0 && (int.len() - i) % 3 == 0 {
            grouped.push(',');
        }
        grouped.push(c);
    }
    format!("{}${}.{}", if neg { "-" } else { "" }, grouped, frac)
}
