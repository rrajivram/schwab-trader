//! Home screen: account bar, do-not-transact panel, and the S&P 500 grouped
//! into collapsible sectors (heaviest first), each with its own sortable table.

use std::cmp::Ordering;

use eframe::egui::{self, CornerRadius, Margin, RichText, Stroke};
use egui_extras::{Column, TableBuilder};
use schwab::{api::MarketData, portfolio::Held, risk::Fit, universe::Constituent};

use crate::app::{group_thousands, money, IndexerApp};
use crate::modes::{Mode, DISCARD_HINT};
use crate::theme::{self, pal, Tone};

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Col {
    Symbol,
    Name,
    Weight,
    Price,
    Change,
    Low52,
    High52,
    Eps,
    Pe,
    Beta,
    Alpha,
    Yield,
    Freq,
    Volume,
    Held,
    Value,
    Return,
}

/// Table columns after the do-not-transact checkbox, in display order.
const COLUMNS: &[(Col, &str, &str)] = &[
    (Col::Symbol, "Symbol", ""),
    (Col::Name, "Name", ""),
    (Col::Weight, "Weight", "Share of the S&P 500 (share classes merged)"),
    (Col::Price, "Price", ""),
    (Col::Change, "Chg %", "Change today"),
    (Col::Low52, "52w Low", ""),
    (Col::High52, "52w High", ""),
    (Col::Eps, "EPS", ""),
    (Col::Pe, "P/E", ""),
    (Col::Beta, "Beta", "How much it moves with the S&P 500: 1.0 = in step, 2.0 = twice as much, 0.5 = half (Schwab)"),
    (Col::Alpha, "Alpha", "Return per year beyond what beta explains, over 3 years of weekly prices vs. SPY. Colored only when it stands out from noise (|t| ≥ 2)."),
    (Col::Yield, "Div Yld", "Dividend yield"),
    (Col::Freq, "Div Freq", "Dividend payments per year"),
    (Col::Volume, "Volume", "Shares traded today"),
    (Col::Held, "Held", "Shares you own"),
    (Col::Value, "Value", "Market value of what you hold"),
    (Col::Return, "Gain/Loss", "Unrealized gain/loss including dividends received"),
];

#[derive(Clone, Copy)]
pub struct SortState {
    pub col: Col,
    pub ascending: bool,
}

impl Default for SortState {
    fn default() -> Self {
        Self { col: Col::Weight, ascending: false }
    }
}

struct Row<'a> {
    c: &'a Constituent,
    md: Option<&'a MarketData>,
    fit: Option<&'a Fit>,
    held: Option<&'a Held>,
    dnt: bool,
    /// Ticked in the mode's selection column (Basket / Discard).
    selected: bool,
    selectable: bool,
    /// Alpha Vantage's readable name when cached, else Schwab's description.
    name: &'a str,
}

impl Row<'_> {
    fn num(&self, col: Col) -> Option<f64> {
        let md = self.md;
        match col {
            Col::Weight => (self.c.weight > 0.0).then_some(self.c.weight),
            Col::Price => md?.price,
            Col::Change => md?.net_percent_change,
            Col::Low52 => md?.w52_low,
            Col::High52 => md?.w52_high,
            Col::Eps => md?.eps,
            Col::Pe => md?.pe_ratio,
            Col::Beta => md?.beta,
            Col::Alpha => self.fit.map(|f| f.alpha),
            Col::Yield => md?.div_yield,
            Col::Freq => md?.div_freq.map(f64::from),
            Col::Volume => md?.volume.map(|v| v as f64),
            Col::Held => self.held.map(|h| h.quantity),
            Col::Value => self.held.map(|h| h.market_value),
            Col::Return => self.held.and_then(Held::gain),
            Col::Symbol | Col::Name => None,
        }
    }

    fn name(&self) -> &str {
        self.name
    }
}

/// Text columns sort alphabetically; numeric columns put missing values last
/// in either direction so blanks never crowd the top.
fn compare(a: &Row, b: &Row, sort: SortState) -> Ordering {
    let dir = |o: Ordering| if sort.ascending { o } else { o.reverse() };
    match sort.col {
        Col::Symbol => dir(a.c.symbol.cmp(&b.c.symbol)),
        Col::Name => dir(a.name().cmp(b.name())),
        col => match (a.num(col), b.num(col)) {
            (Some(x), Some(y)) => dir(x.total_cmp(&y)),
            (Some(_), None) => Ordering::Less,
            (None, Some(_)) => Ordering::Greater,
            (None, None) => Ordering::Equal,
        },
    }
    .then_with(|| a.c.symbol.cmp(&b.c.symbol))
}

/// Tooltip for an alpha figure: what it means and how much to trust it.
pub fn fit_hint(f: &Fit) -> String {
    let verdict = if f.significant() { "stands out from noise" } else { "can't be told from noise" };
    format!(
        "Alpha {:+.1}%/yr · t = {:.1} ({verdict})\nBeta {:.2} · R² {:.2} · {} weeks vs. SPY\n\nPrice-only returns: dividends aren't included.",
        f.alpha * 100.0,
        f.alpha_t,
        f.beta,
        f.r2,
        f.weeks
    )
}

fn freq_label(freq: Option<u32>) -> &'static str {
    match freq {
        Some(12) => "Monthly",
        Some(4) => "Quarterly",
        Some(2) => "Semi-annual",
        Some(1) => "Annual",
        _ => "—",
    }
}

fn opt(v: Option<f64>, f: impl Fn(f64) -> String) -> String {
    v.map(f).unwrap_or_else(|| "—".into())
}

/// Card for holdings that aren't S&P 500 constituents.
const OTHER_SECTION: &str = "ETFs & other holdings";

enum Action {
    SetDnt(String, bool),
    Select(String, bool),
    Sort(String, Col),
}

impl IndexerApp {
    pub(crate) fn home_ui(&mut self, ui: &mut egui::Ui, frame: &eframe::Frame) {
        self.top_bar(ui);
        self.settings_modal(ui);
        match self.mode {
            Mode::Review => return self.review_ui(ui),
            Mode::Execute => return self.execute_ui(ui),
            Mode::Bonds => return self.bonds_ui(ui),
            Mode::Plan => return self.plan_ui(ui, frame),
            _ => {}
        }
        self.summary_strip(ui);
        if self.show_dnt_panel {
            self.dnt_panel(ui);
        }
        match self.mode {
            Mode::Create => self.create_panel(ui),
            Mode::Rebalance => self.rebalance_panel(ui),
            Mode::Browse | Mode::Review | Mode::Execute | Mode::Bonds | Mode::Plan => {}
        }
        egui::CentralPanel::default()
            .frame(egui::Frame::new().fill(pal(ui).bg).inner_margin(Margin::symmetric(18, 10)))
            .show(ui, |ui| self.sectors_ui(ui));
    }

    fn top_bar(&mut self, ui: &mut egui::Ui) {
        let p = pal(ui);
        let frame = egui::Frame::new()
            .fill(p.surface)
            .stroke(Stroke::new(1.0, p.border))
            .inner_margin(Margin::symmetric(18, 10));
        egui::Panel::top("app_bar").frame(frame).show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.label(RichText::new("Indexer").font(theme::sans_semibold(18.0)).color(p.ink));
                theme::tone_pill(ui, "S&P 500", Tone::Accent);
                ui.add_space(18.0);
                self.mode_switcher(ui);

                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.button("Log in again").clicked() {
                        self.relogin();
                    }
                    if ui.button("Settings").clicked() {
                        let key = schwab::config::Config::load().ok().and_then(|c| c.alphavantage_key).unwrap_or_default();
                        let plan_page = schwab::config::Config::load()
                            .ok()
                            .and_then(|c| c.plan_page)
                            .unwrap_or_else(|| crate::plan_tab::DEFAULT_PLAN_PAGE.to_string());
                        self.settings = Some(crate::app::SettingsForm { plan_page, av_key: key, saved: false });
                    }
                    let busy = self.account.loading || self.market.loading || self.universe.loading;
                    let locked = self.mode == Mode::Execute;
                    if ui
                        .add_enabled(!busy && !locked, egui::Button::new("Update index"))
                        .on_hover_text("Re-download S&P 500 weights and sectors from State Street")
                        .clicked()
                    {
                        self.load_universe(true);
                    }
                    if ui
                        .add_enabled(!busy && !locked, egui::Button::new("Refresh quotes"))
                        .on_hover_text("Reload your account, positions and all quotes")
                        .clicked()
                    {
                        self.load_account();
                        self.load_market();
                    }
                    let dnt = format!("Do not transact · {}", self.dnt.len());
                    ui.add_enabled_ui(!locked, |ui| ui.toggle_value(&mut self.show_dnt_panel, dnt));
                });
            });
        });
    }

    fn settings_modal(&mut self, ui: &mut egui::Ui) {
        let Some(form) = &mut self.settings else { return };
        let p = pal(ui);
        let mut close = false;
        let used = self.av_used_today;
        let resp = egui::Modal::new(egui::Id::new("settings")).show(ui.ctx(), |ui| {
            ui.set_width(440.0);
            ui.label(RichText::new("Settings").font(theme::sans_semibold(theme::HEADING)));
            ui.add_space(10.0);
            ui.label(theme::eyebrow(ui, "Alpha Vantage API key"));
            ui.label(RichText::new("Used for forward P/E, analyst targets and ratings, and company names in Review. Saved on this Mac only.").color(p.muted));
            ui.add(egui::TextEdit::singleline(&mut form.av_key).password(true).desired_width(f32::INFINITY));
            ui.horizontal(|ui| {
                ui.label(
                    RichText::new(format!("{used} of {} free requests used today", schwab::alphavantage::DAILY_LIMIT))
                        .color(p.muted)
                        .font(theme::sans(theme::SMALL)),
                );
                ui.hyperlink_to(RichText::new("Get a key").font(theme::sans(theme::SMALL)), "https://www.alphavantage.co/support/#api-key");
            });
            ui.add_space(10.0);
            ui.label(theme::eyebrow(ui, "Plan tab page"));
            ui.label(RichText::new("The local HTML file shown in the Plan tab.").color(p.muted));
            ui.add(egui::TextEdit::singleline(&mut form.plan_page).desired_width(f32::INFINITY));
            if form.saved {
                ui.label(RichText::new("Saved").color(p.gain));
            }
            ui.add_space(12.0);
            ui.horizontal(|ui| {
                close = ui.button("Close").clicked();
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.add(theme::primary(ui, "Save")).clicked() {
                        let mut cfg = schwab::config::Config::load().unwrap_or_default();
                        let key = form.av_key.trim().to_string();
                        cfg.alphavantage_key = (!key.is_empty()).then_some(key);
                        let page = form.plan_page.trim().to_string();
                        cfg.plan_page = (!page.is_empty()).then_some(page);
                        form.saved = cfg.save().is_ok();
                    }
                });
            });
        });
        if close || resp.should_close() {
            self.settings = None;
        }
    }

    /// Browse | Create | Rebalance | Bonds, as one segmented control.
    fn mode_switcher(&mut self, ui: &mut egui::Ui) {
        let p = pal(ui);
        let current = match self.mode {
            Mode::Review | Mode::Execute => {
                if self.review.as_ref().is_some_and(|r| r.rebalance) { Mode::Rebalance } else { Mode::Create }
            }
            m => m,
        };
        let enabled = self.universe.value.is_some() && self.mode != Mode::Execute;
        let soon = crate::bonds_tab::soon_count(self);
        egui::Frame::new()
            .fill(p.neutral_soft)
            .corner_radius(CornerRadius::same(8))
            .inner_margin(Margin::same(3))
            .show(ui, |ui| {
                ui.spacing_mut().item_spacing.x = 2.0;
                ui.horizontal(|ui| {
                    for (mode, label, hint) in [
                        (Mode::Browse, "Browse", "Explore the index"),
                        (Mode::Create, "Create", "Build a new basket"),
                        (Mode::Rebalance, "Rebalance", "Replace holdings that are below cost"),
                        (Mode::Bonds, "Bonds", "Maturities and coupons coming your way"),
                        (Mode::Plan, "Plan", "Your divestiture plan page"),
                    ] {
                        let label = if mode == Mode::Bonds && soon > 0 { format!("Bonds · {soon}") } else { label.to_string() };
                        let hint = if mode == Mode::Bonds && soon > 0 {
                            format!("{soon} payment{} in the next 30 days", if soon == 1 { "" } else { "s" })
                        } else {
                            hint.to_string()
                        };
                        // Bonds only need the account, not the index data.
                        let enabled = if matches!(mode, Mode::Bonds | Mode::Plan) { self.mode != Mode::Execute } else { enabled };
                        let on = current == mode;
                        let text = RichText::new(label)
                            .font(if on { theme::sans_semibold(theme::BODY) } else { theme::sans_medium(theme::BODY) })
                            .color(if on { p.ink } else { p.muted });
                        let button = egui::Button::new(text)
                            .fill(if on { p.surface } else { p.neutral_soft })
                            .stroke(if on { Stroke::new(1.0, p.border) } else { Stroke::NONE })
                            .corner_radius(CornerRadius::same(6))
                            .min_size(egui::Vec2::new(86.0, 26.0));
                        if ui.add_enabled(enabled, button).on_hover_text(hint).clicked() && !on {
                            self.mode = mode;
                        }
                    }
                });
            });
    }

    fn summary_strip(&mut self, ui: &mut egui::Ui) {
        let p = pal(ui);
        egui::Panel::top("summary").frame(egui::Frame::new().fill(p.bg).inner_margin(Margin { left: 18, right: 18, top: 14, bottom: 4 })).show(ui, |ui| {
            theme::card(ui).inner_margin(Margin::symmetric(18, 12)).show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 34.0;
                    match &self.account.value {
                        Some(acct) => {
                            theme::stat(ui, "Cash", &money(acct.cash_balance), None);
                            // Everything in the account, by kind, so the pieces add
                            // up to Schwab's account value.
                            let by_kind = |f: &dyn Fn(&str) -> bool| -> (f64, usize) {
                                let ps: Vec<_> = acct.positions.iter().filter(|x| x.long_quantity > 0.0 && f(&x.asset_type)).collect();
                                (ps.iter().map(|x| x.market_value).sum(), ps.len())
                            };
                            let (stocks, n_stocks) = by_kind(&|t| t == "EQUITY");
                            let (bonds, n_bonds) = by_kind(&|t| t == "FIXED_INCOME");
                            let (funds, n_funds) = by_kind(&|t| t != "EQUITY" && t != "FIXED_INCOME");
                            theme::stat(ui, &format!("Stocks · {n_stocks}"), &money(stocks), None);
                            theme::stat(ui, &format!("ETFs & funds · {n_funds}"), &money(funds), None);
                            theme::stat(ui, &format!("Bonds · {n_bonds}"), &money(bonds), None);
                            theme::stat(ui, "Account value", &money(acct.liquidation_value), Some(p.accent));
                            let market = self.market.value.as_ref();
                            let beta = schwab::risk::weighted_beta(
                                self.held.iter().map(|(s, h)| (h.market_value, market.and_then(|m| m.get(s)).and_then(|m| m.beta))),
                            );
                            if let Some(b) = beta {
                                let account = b.diluted(acct.liquidation_value).map(|a| format!("{a:.2}")).unwrap_or("—".into());
                                theme::stat(ui, &format!("Beta · {} holdings", b.count), &format!("{:.2}", b.beta), None)
                                    .on_hover_text(format!(
                                        "Value-weighted beta of the {} stocks and ETFs with a Schwab beta ({}). \
                                         If the market falls 10%, expect these to fall about {:.0}%.\n\n\
                                         Counting cash and bonds as 0, the whole account's beta is {account}.",
                                        b.count, money(b.covered), b.beta * 10.0
                                    ));
                            }
                            // Gain/loss only where Schwab reports a cost basis.
                            let known: Vec<f64> = self.held.values().filter_map(Held::gain).collect();
                            let unknown = self.held.len() - known.len();
                            if known.is_empty() {
                                theme::stat(ui, "Gain / loss", "—", Some(p.muted));
                            } else {
                                let ret: f64 = known.iter().sum();
                                let color = if ret >= 0.0 { p.gain } else { p.loss };
                                theme::stat(ui, &format!("Gain / loss · {} of {} holdings", known.len(), self.held.len()), &money(ret), Some(color));
                            }
                            if unknown > 0 {
                                ui.label(RichText::new(format!("{unknown} holdings have no cost basis from Schwab")).color(p.muted).font(theme::sans(theme::SMALL)))
                                    .on_hover_text("Schwab's API reports an average price of 0 for these (usually shares transferred in), so their gain or loss can't be computed here.");
                            }
                        }
                        None if self.account.loading => {
                            ui.spinner();
                            ui.label("Loading account…");
                        }
                        None => {
                            ui.label("No account loaded");
                        }
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.spacing_mut().item_spacing.x = 8.0;
                        let n = &self.account.value.as_ref().map(|a| a.account_number.clone()).unwrap_or_default();
                        if !n.is_empty() {
                            ui.label(RichText::new(format!("Account ···{}", &n[n.len().saturating_sub(4)..])).color(p.muted));
                        }
                        if let Some(u) = &self.universe.value {
                            let date = u.source_as_of.as_deref().unwrap_or("").trim_start_matches("As of ").to_string();
                            theme::tone_pill(ui, &format!("Weights {date}"), Tone::Neutral);
                        }
                        if self.market.loading || self.universe.loading {
                            ui.spinner();
                            ui.label(RichText::new("Loading quotes…").color(p.muted));
                        } else if let Some(m) = &self.market.value {
                            theme::tone_pill(ui, &format!("{} quotes", m.len()), Tone::Neutral);
                        }
                        if self.dividends.loading {
                            ui.label(RichText::new("Loading dividends…").color(p.muted));
                        }
                        if self.history_running {
                            let (done, total) = self.history_progress;
                            ui.spinner();
                            ui.label(RichText::new(format!("Price history {done}/{total}")).color(p.muted))
                                .on_hover_text("Weekly prices for alpha, about 2 per second to stay under Schwab's rate limit. Cached for a week.");
                        }
                    });
                });
            });

            for (label, e) in [
                ("Account", &self.account.error),
                ("Index data", &self.universe.error),
                ("Quotes", &self.market.error),
                ("Dividends", &self.dividends.error),
                ("Risk-free rate (needed for alpha)", &self.risk_free.error),
                ("Do-not-transact list", &self.dnt_error),
            ] {
                if let Some(e) = e {
                    ui.colored_label(p.loss, format!("{label}: {e}"));
                }
            }
            if !self.history_failed.is_empty() {
                let mut f = self.history_failed.clone();
                f.sort();
                let shown = f.iter().take(8).cloned().collect::<Vec<_>>().join(", ");
                let more = if f.len() > 8 { format!(" and {} more", f.len() - 8) } else { String::new() };
                ui.label(RichText::new(format!("No price history for {shown}{more}; their alpha is blank.")).color(p.warn).font(theme::sans(theme::SMALL)));
            }
            if let Some(u) = &self.universe.value {
                for w in &u.warnings {
                    ui.label(RichText::new(w).color(p.warn).font(theme::sans(theme::SMALL)));
                }
            }
        });
    }

    fn dnt_panel(&mut self, ui: &mut egui::Ui) {
        let p = pal(ui);
        let mut remove = None;
        let mut add = None;
        let names = self.names(&self.dnt);
        let frame = egui::Frame::new().fill(p.surface).stroke(Stroke::new(1.0, p.border)).inner_margin(Margin::same(16));
        egui::Panel::right("dnt_panel").frame(frame).default_size(260.0).show(ui, |ui| {
            ui.label(RichText::new("Do not transact").font(theme::sans_semibold(theme::TITLE)));
            ui.label(RichText::new("Create and Rebalance never buy or sell these.").color(p.muted));
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                let resp = ui.add(egui::TextEdit::singleline(&mut self.dnt_input).hint_text("Ticker, e.g. TSLA").desired_width(130.0));
                let enter = resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                if (ui.add(theme::primary(ui, "Add")).clicked() || enter) && !self.dnt_input.trim().is_empty() {
                    add = Some(self.dnt_input.trim().to_uppercase());
                }
            });
            ui.add_space(6.0);
            ui.separator();
            egui::ScrollArea::vertical().show(ui, |ui| {
                if self.dnt.is_empty() {
                    ui.label(RichText::new("Nothing blocked yet. Use a row's Block button or add a ticker above.").color(p.muted));
                }
                for sym in &self.dnt {
                    ui.horizontal(|ui| {
                        ui.label(RichText::new(sym).font(theme::mono_semibold(theme::BODY)));
                        let name = &names[sym];
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if ui.small_button("Remove").clicked() {
                                remove = Some(sym.clone());
                            }
                            ui.add(egui::Label::new(RichText::new(name).color(p.muted)).truncate()).on_hover_text(name);
                        });
                    });
                }
            });
        });
        if let Some(sym) = add {
            self.set_dnt(&sym, true);
            self.dnt_input.clear();
        }
        if let Some(sym) = remove {
            self.set_dnt(&sym, false);
        }
    }

    fn sectors_ui(&mut self, ui: &mut egui::Ui) {
        let p = pal(ui);
        let Some(universe) = &self.universe.value else {
            if self.universe.loading {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label("Downloading S&P 500 weights and sectors…");
                });
            }
            return;
        };

        let empty = Default::default();
        let market = self.market.value.as_ref().unwrap_or(&empty);
        let mut actions = Vec::new();
        let sectors = universe.sectors_by_weight();
        let max_weight = sectors.first().map_or(1.0, |(_, w)| *w);

        let in_index: std::collections::HashSet<&str> = universe.constituents.iter().map(|c| c.symbol.as_str()).collect();
        let mut other_syms: Vec<&String> = self.held.keys().filter(|s| !in_index.contains(s.as_str())).collect();
        other_syms.sort();
        let others: Vec<Constituent> = other_syms
            .into_iter()
            .map(|s| Constituent { symbol: s.clone(), sector: OTHER_SECTION.to_string(), weight: 0.0, merged: Vec::new() })
            .collect();

        egui::ScrollArea::both().auto_shrink([false, false]).show(ui, |ui| {
            ui.spacing_mut().item_spacing.y = 10.0;
            // Holdings outside the S&P 500 (ETFs, funds, other stocks) get their
            // own card, laid out like a sector.
            if matches!(self.mode, Mode::Browse | Mode::Rebalance) && !others.is_empty() {
                let mut rows: Vec<Row> = others
                    .iter()
                    .map(|c| {
                        let md = market.get(&c.symbol);
                        let name = md.and_then(|m| m.description.as_deref()).unwrap_or("");
                        Row { c, md, fit: self.fits.get(&c.symbol), held: self.held.get(&c.symbol), dnt: self.dnt.contains(&c.symbol), selected: false, selectable: false, name }
                    })
                    .collect();
                let sort = self.sort.get(OTHER_SECTION).copied().unwrap_or(SortState { col: Col::Value, ascending: false });
                rows.sort_by(|a, b| compare(a, b, sort));
                let value: f64 = rows.iter().filter_map(|r| r.held).map(|h| h.market_value).sum();
                theme::card(ui).inner_margin(Margin::symmetric(14, 8)).show(ui, |ui| {
                    let id = ui.make_persistent_id(("sector", OTHER_SECTION));
                    egui::collapsing_header::CollapsingState::load_with_default_open(ui.ctx(), id, true)
                        .show_header(ui, |ui| {
                            ui.label(RichText::new(OTHER_SECTION).font(theme::sans_semibold(theme::TITLE)).color(p.ink));
                            ui.add_space(8.0);
                            ui.label(RichText::new(money(value)).font(theme::mono_medium(theme::BODY)));
                            ui.label(RichText::new(format!("{} positions outside the S&P 500", rows.len())).color(p.muted));
                        })
                        .body_unindented(|ui| {
                            ui.add_space(4.0);
                            // Browse layout: these can't be added to a basket or swapped by Rebalance.
                            sector_table(ui, OTHER_SECTION, &rows, sort, Mode::Browse, &mut actions);
                        });
                });
            }

            for (sector, sector_weight) in &sectors {
                let mut rows: Vec<Row> = universe
                    .constituents
                    .iter()
                    .filter(|c| &c.sector == sector)
                    .map(|c| {
                        let held = self.held.get(&c.symbol);
                        let dnt = self.dnt.contains(&c.symbol);
                        let (selected, selectable) = match self.mode {
                            Mode::Create => (self.basket.contains(&c.symbol), !dnt),
                            Mode::Rebalance => (
                                self.discards.contains(&c.symbol),
                                !dnt && held.is_some_and(Held::is_losing),
                            ),
                            Mode::Browse | Mode::Review | Mode::Execute | Mode::Bonds | Mode::Plan => (false, false),
                        };
                        let md = market.get(&c.symbol);
                        let name = self
                            .overviews
                            .get(&c.symbol)
                            .and_then(|o| o.name.as_deref())
                            .or_else(|| md.and_then(|m| m.description.as_deref()))
                            .unwrap_or("");
                        Row { c, md, fit: self.fits.get(&c.symbol), held, dnt, selected, selectable, name }
                    })
                    .collect();
                let sort = self.sort.get(sector).copied().unwrap_or_default();
                rows.sort_by(|a, b| compare(a, b, sort));
                // Browse and Rebalance: held positions always lead their sector.
                // The sort is stable, so each group keeps the column order.
                if matches!(self.mode, Mode::Browse | Mode::Rebalance) {
                    rows.sort_by_key(|r| r.held.is_none());
                }
                let n_held = rows.iter().filter(|r| r.held.is_some()).count();
                let n_losing = rows.iter().filter(|r| r.held.is_some_and(Held::is_losing)).count();
                let n_selected = rows.iter().filter(|r| r.selected).count();

                theme::card(ui).inner_margin(Margin::symmetric(14, 8)).show(ui, |ui| {
                    let id = ui.make_persistent_id(("sector", sector.as_str()));
                    egui::collapsing_header::CollapsingState::load_with_default_open(ui.ctx(), id, false)
                        .show_header(ui, |ui| {
                            ui.label(RichText::new(sector.as_str()).font(theme::sans_semibold(theme::TITLE)).color(p.ink));
                            ui.add_space(8.0);
                            theme::weight_bar(ui, (*sector_weight / max_weight) as f32, 110.0);
                            ui.label(RichText::new(format!("{:.1}%", sector_weight * 100.0)).font(theme::mono_medium(theme::BODY)));
                            ui.label(RichText::new(format!("{} stocks", rows.len())).color(p.muted));
                            if n_held > 0 {
                                let all_unknown = rows.iter().filter_map(|r| r.held).all(|h| h.cost_unknown);
                                let tone = if n_losing > 0 { Tone::Loss } else if all_unknown { Tone::Neutral } else { Tone::Gain };
                                theme::tone_pill(ui, &format!("{n_held} held"), tone);
                            }
                            if n_selected > 0 {
                                let label = if self.mode == Mode::Rebalance { "discarding" } else { "in basket" };
                                theme::tone_pill(ui, &format!("{n_selected} {label}"), Tone::Accent);
                            }
                        })
                        .body_unindented(|ui| {
                            ui.add_space(4.0);
                            sector_table(ui, sector, &rows, sort, self.mode, &mut actions);
                        });
                });
            }
        });

        for action in actions {
            match action {
                Action::SetDnt(sym, on) => self.set_dnt(&sym, on),
                Action::Select(sym, on) => {
                    let list = if self.mode == Mode::Rebalance { &mut self.discards } else { &mut self.basket };
                    list.retain(|s| *s != sym);
                    if on {
                        list.push(sym);
                    }
                }
                Action::Sort(sector, col) => {
                    let s = self.sort.entry(sector).or_default();
                    if s.col == col {
                        s.ascending = !s.ascending;
                    } else {
                        // Text sorts A→Z first; numbers biggest first.
                        *s = SortState { col, ascending: matches!(col, Col::Symbol | Col::Name) };
                    }
                }
            }
        }
    }
}

fn sector_table(ui: &mut egui::Ui, sector: &str, rows: &[Row], sort: SortState, mode: Mode, actions: &mut Vec<Action>) {
    let select = match mode {
        Mode::Create => Some(("Basket", "Add", "✓ In basket", "Click to remove from the basket")),
        Mode::Rebalance => Some(("Discard", "Discard", "✓ Discarding", "Click to keep this holding")),
        Mode::Browse | Mode::Review | Mode::Execute | Mode::Bonds | Mode::Plan => None,
    };
    let p = pal(ui);
    let mut table = TableBuilder::new(ui)
        .id_salt(sector)
        .striped(true)
        .vscroll(false)
        .resizable(true)
        .cell_layout(egui::Layout::left_to_right(egui::Align::Center));
    if select.is_some() {
        table = table.column(Column::exact(104.0));
    }
    table = table.column(Column::exact(76.0));
    for (col, _, _) in COLUMNS {
        table = table.column(match col {
            Col::Name => Column::initial(210.0).clip(true),
            Col::Symbol => Column::initial(70.0),
            Col::Freq => Column::initial(92.0),
            Col::Volume | Col::Return | Col::Value => Column::initial(104.0),
            _ => Column::initial(80.0),
        });
    }

    table
        .header(26.0, |mut header| {
            if let Some((label, ..)) = select {
                header.col(|ui| {
                    ui.label(theme::eyebrow(ui, label));
                });
            }
            header.col(|ui| {
                ui.label(theme::eyebrow(ui, "Block")).on_hover_text("Do not transact: never bought or sold by Create/Rebalance");
            });
            for (col, label, hover) in COLUMNS {
                header.col(|ui| {
                    let active = (sort.col == *col).then_some(sort.ascending);
                    if theme::sort_header(ui, label, active, hover) {
                        actions.push(Action::Sort(sector.to_string(), *col));
                    }
                });
            }
        })
        .body(|mut body| {
            for r in rows {
                body.row(26.0, |mut row| {
                    // Held rows: light green/red background across the whole row
                    // (above/below cost incl. dividends) plus a stronger left stripe.
                    let (stripe, tint) = match r.held {
                        Some(h) if h.cost_unknown => (Some(p.muted), Some(p.neutral_soft)),
                        Some(h) if h.is_losing() => (Some(p.loss), Some(p.loss_soft)),
                        Some(_) => (Some(p.gain), Some(p.gain_soft)),
                        None => (None, None),
                    };
                    let mut first = true;
                    let mut mark = |ui: &mut egui::Ui| {
                        if let Some(bg) = tint {
                            theme::row_tint(ui, bg);
                        }
                        if first {
                            if let Some(c) = stripe {
                                theme::left_stripe(ui, c);
                            }
                            first = false;
                        }
                    };
                    if let Some((_, off, on, remove_hint)) = select {
                        row.col(|ui| {
                            mark(ui);
                            let tone = if mode == Mode::Rebalance { Tone::Loss } else { Tone::Accent };
                            let resp = theme::toggle_button(ui, r.selected, off, on, tone, r.selectable);
                            let resp = if r.selected { resp.on_hover_text(remove_hint) } else { resp };
                            let resp = if mode == Mode::Rebalance {
                                resp.on_disabled_hover_text(DISCARD_HINT)
                            } else {
                                resp.on_disabled_hover_text("Blocked: on the do-not-transact list")
                            };
                            if resp.clicked() {
                                actions.push(Action::Select(r.c.symbol.clone(), !r.selected));
                            }
                        });
                    }
                    row.col(|ui| {
                        mark(ui);
                        let resp = theme::toggle_button(ui, r.dnt, "Block", "Blocked", Tone::Warn, true).on_hover_text(if r.dnt {
                            "On the do-not-transact list. Click to unblock."
                        } else {
                            "Add to the do-not-transact list"
                        });
                        if resp.clicked() {
                            actions.push(Action::SetDnt(r.c.symbol.clone(), !r.dnt));
                        }
                    });
                    for (col, _, _) in COLUMNS {
                        row.col(|ui| {
                            mark(ui);
                            cell(ui, r, *col);
                        });
                    }
                });
            }
        });
}

fn right(ui: &mut egui::Ui, text: RichText) {
    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
        ui.label(text);
    });
}

fn cell(ui: &mut egui::Ui, r: &Row, col: Col) {
    let p = pal(ui);
    let md = r.md;
    let figure = |s: String| RichText::new(s).font(theme::mono(12.5)).color(p.ink);
    match col {
        Col::Symbol => {
            let resp = ui.label(RichText::new(&r.c.symbol).font(theme::mono_semibold(13.0)).color(p.ink));
            if !r.c.merged.is_empty() {
                resp.on_hover_text(format!("Includes {}", r.c.merged.join(", ")));
            }
        }
        Col::Name => {
            ui.label(RichText::new(r.name()).color(p.ink)).on_hover_text(r.name());
        }
        Col::Return => {
            if let Some(h) = r.held.filter(|h| h.cost_unknown) {
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.label(RichText::new("no cost basis").color(p.muted).font(theme::sans(theme::SMALL))).on_hover_text(format!(
                        "Schwab reports no purchase price for this position, so its gain or loss is unknown. Value {}.",
                        money(h.market_value)
                    ));
                });
            } else if let Some(h) = r.held {
                let v = h.total_return();
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let tone = if v >= 0.0 { Tone::Gain } else { Tone::Loss };
                    theme::tone_pill(ui, &money(v), tone).on_hover_text(format!(
                        "Value {}  +  dividends {}  −  cost {}",
                        money(h.market_value),
                        money(h.dividends),
                        money(h.cost)
                    ));
                });
            }
        }
        Col::Change => {
            if let Some(v) = md.and_then(|m| m.net_percent_change) {
                let color = if v >= 0.0 { p.gain } else { p.loss };
                right(ui, RichText::new(format!("{v:+.2}%")).font(theme::mono(12.5)).color(color));
            }
        }
        Col::Freq => {
            ui.label(RichText::new(freq_label(md.and_then(|m| m.div_freq))).color(p.muted));
        }
        Col::Held => {
            if let Some(h) = r.held {
                right(ui, RichText::new(fmt_qty(h.quantity)).font(theme::mono_semibold(12.5)).color(p.ink));
            }
        }
        Col::Value => {
            if let Some(h) = r.held {
                right(ui, RichText::new(money(h.market_value)).font(theme::mono(12.5)).color(p.ink));
            }
        }
        // Not in the index (ETFs, other holdings): no weight to show.
        Col::Weight if r.c.weight <= 0.0 => right(ui, figure("—".into())),
        Col::Weight => right(ui, figure(format!("{:.3}%", r.c.weight * 100.0))),
        Col::Price | Col::Low52 | Col::High52 | Col::Eps => right(ui, figure(opt(r.num(col), |v| format!("{v:.2}")))),
        Col::Pe => right(ui, figure(opt(r.num(col), |v| format!("{v:.1}")))),
        Col::Beta => right(ui, figure(opt(r.num(col), |v| format!("{v:.2}")))),
        Col::Alpha => match r.fit {
            Some(f) => {
                let color = match (f.significant(), f.alpha >= 0.0) {
                    (false, _) => p.muted,
                    (true, true) => p.gain,
                    (true, false) => p.loss,
                };
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.label(RichText::new(format!("{:+.1}%", f.alpha * 100.0)).font(theme::mono(12.5)).color(color))
                        .on_hover_text(fit_hint(f));
                });
            }
            None => right(ui, figure("—".into())),
        },
        Col::Yield => right(ui, figure(opt(r.num(col), |v| format!("{v:.2}%")))),
        Col::Volume => right(ui, figure(opt(r.num(col), |v| group_thousands(&format!("{v:.0}"))))),
    }
}

/// Share counts: whole numbers plainly, fractions to 4 places, no trailing zeros.
pub fn fmt_qty(q: f64) -> String {
    let s = format!("{q:.4}");
    s.trim_end_matches('0').trim_end_matches('.').to_string()
}
