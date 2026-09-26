//! Home screen: account bar, do-not-transact panel, and the S&P 500 grouped
//! into collapsible sectors (heaviest first), each with its own sortable table.

use std::cmp::Ordering;

use eframe::egui::{self, Color32, RichText};
use egui_extras::{Column, TableBuilder};
use schwab::{api::MarketData, portfolio::Held, universe::Constituent};

use crate::app::{group_thousands, money, ticker_with_name, IndexerApp};
use crate::modes::{Mode, DISCARD_HINT};

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
    Yield,
    Freq,
    Volume,
    Held,
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
    (Col::Yield, "Div Yld", "Dividend yield"),
    (Col::Freq, "Div Freq", "Dividend payments per year"),
    (Col::Volume, "Volume", "Shares traded today"),
    (Col::Held, "Held", "Shares you own"),
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
    held: Option<&'a Held>,
    dnt: bool,
    /// Ticked in the mode's selection column (Basket / Discard).
    selected: bool,
    selectable: bool,
}

impl Row<'_> {
    fn num(&self, col: Col) -> Option<f64> {
        let md = self.md;
        match col {
            Col::Weight => Some(self.c.weight),
            Col::Price => md?.price,
            Col::Change => md?.net_percent_change,
            Col::Low52 => md?.w52_low,
            Col::High52 => md?.w52_high,
            Col::Eps => md?.eps,
            Col::Pe => md?.pe_ratio,
            Col::Yield => md?.div_yield,
            Col::Freq => md?.div_freq.map(f64::from),
            Col::Volume => md?.volume.map(|v| v as f64),
            Col::Held => self.held.map(|h| h.quantity),
            Col::Return => self.held.map(Held::total_return),
            Col::Symbol | Col::Name => None,
        }
    }

    fn name(&self) -> &str {
        self.md.and_then(|m| m.description.as_deref()).unwrap_or("")
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

fn gain_color(ui: &egui::Ui, v: f64) -> Color32 {
    let dark = ui.visuals().dark_mode;
    match (v >= 0.0, dark) {
        (true, true) => Color32::from_rgb(110, 200, 120),
        (true, false) => Color32::from_rgb(20, 120, 40),
        (false, true) => Color32::from_rgb(235, 110, 100),
        (false, false) => Color32::from_rgb(180, 30, 30),
    }
}

/// Faint row background for held positions.
fn row_tint(ui: &egui::Ui, held: Option<&Held>) -> Option<Color32> {
    let h = held?;
    let alpha = if ui.visuals().dark_mode { 40 } else { 30 };
    Some(if h.is_losing() {
        Color32::from_rgba_unmultiplied(220, 50, 40, alpha)
    } else {
        Color32::from_rgba_unmultiplied(40, 180, 70, alpha)
    })
}

enum Action {
    SetDnt(String, bool),
    Select(String, bool),
    Sort(String, Col),
}

impl IndexerApp {
    pub(crate) fn home_ui(&mut self, ui: &mut egui::Ui) {
        self.top_bar(ui);
        match self.mode {
            Mode::Review => return self.review_ui(ui),
            Mode::Execute => return self.execute_ui(ui),
            _ => {}
        }
        if self.show_dnt_panel {
            self.dnt_panel(ui);
        }
        match self.mode {
            Mode::Create => self.create_panel(ui),
            Mode::Rebalance => self.rebalance_panel(ui),
            Mode::Browse | Mode::Review | Mode::Execute => {}
        }
        egui::CentralPanel::default().show(ui, |ui| self.sectors_ui(ui));
    }

    fn top_bar(&mut self, ui: &mut egui::Ui) {
        egui::Panel::top("account_bar").show(ui, |ui| {
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                match (&self.account.value, self.account.loading) {
                    (_, true) => {
                        ui.spinner();
                        ui.label("Loading account…");
                    }
                    (Some(acct), false) => {
                        let n = &acct.account_number;
                        ui.label(format!("Account …{}", &n[n.len().saturating_sub(4)..]));
                        ui.separator();
                        ui.label(RichText::new(format!("Cash {}", money(acct.cash_balance))).strong().size(16.0));
                    }
                    (None, false) => {
                        ui.label("No account loaded");
                    }
                }
                if self.dividends.loading {
                    ui.separator();
                    ui.spinner();
                    ui.label("Loading dividend history…");
                }

                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.button("Log in again").clicked() {
                        self.relogin();
                    }
                    let busy = self.account.loading || self.market.loading || self.universe.loading;
                    if ui
                        .add_enabled(!busy, egui::Button::new("Refresh index data"))
                        .on_hover_text("Re-download S&P 500 weights and sectors from State Street")
                        .clicked()
                    {
                        self.load_universe(true);
                    }
                    if ui
                        .add_enabled(!busy, egui::Button::new("Refresh"))
                        .on_hover_text("Reload account, positions and quotes")
                        .clicked()
                    {
                        self.load_account();
                        self.load_market();
                    }
                    ui.toggle_value(&mut self.show_dnt_panel, format!("Do not transact ({})", self.dnt.len()));
                    ui.separator();
                    // No switching modes while orders are being placed/tracked.
                    let ready = self.universe.value.is_some() && self.mode != Mode::Execute;
                    for (mode, label, hover) in [
                        (Mode::Rebalance, "Rebalance", "Replace losing holdings"),
                        (Mode::Create, "Create", "Build a new basket"),
                    ] {
                        let active = self.mode == mode
                            || (self.mode == Mode::Review && self.review.as_ref().is_some_and(|r| r.rebalance == (mode == Mode::Rebalance)));
                        let resp = ui.add_enabled(ready, egui::Button::selectable(active, label)).on_hover_text(hover);
                        if resp.clicked() {
                            self.mode = if active { Mode::Browse } else { mode };
                        }
                    }
                });
            });

            let err = ui.visuals().error_fg_color;
            let warn = ui.visuals().warn_fg_color;
            for (label, e) in [
                ("Account", &self.account.error),
                ("Index data", &self.universe.error),
                ("Quotes", &self.market.error),
                ("Dividends", &self.dividends.error),
                ("Do-not-transact", &self.dnt_error),
            ] {
                if let Some(e) = e {
                    ui.colored_label(err, format!("{label}: {e}"));
                }
            }
            if let Some(u) = &self.universe.value {
                for w in &u.warnings {
                    ui.colored_label(warn, w);
                }
            }
            ui.add_space(4.0);
        });
    }

    fn dnt_panel(&mut self, ui: &mut egui::Ui) {
        let mut remove = None;
        let mut add = None;
        let names = self.names(&self.dnt);
        egui::Panel::right("dnt_panel").default_size(220.0).show(ui, |ui| {
            ui.heading("Do not transact");
            ui.label("Never bought or sold by Create/Rebalance.");
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                let resp = ui.add(egui::TextEdit::singleline(&mut self.dnt_input).hint_text("Symbol").desired_width(100.0));
                let enter = resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                if (ui.button("Add").clicked() || enter) && !self.dnt_input.trim().is_empty() {
                    add = Some(self.dnt_input.trim().to_uppercase());
                }
            });
            ui.separator();
            egui::ScrollArea::vertical().show(ui, |ui| {
                if self.dnt.is_empty() {
                    ui.weak("Empty — tick a row's box to add it.");
                }
                for sym in &self.dnt {
                    ui.horizontal(|ui| {
                        if ui.small_button("✕").on_hover_text("Remove").clicked() {
                            remove = Some(sym.clone());
                        }
                        ticker_with_name(ui, sym, &names[sym]);
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
        let Some(universe) = &self.universe.value else {
            if self.universe.loading {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label("Downloading S&P 500 weights and sectors…");
                });
            }
            return;
        };

        ui.horizontal(|ui| {
            ui.weak(format!(
                "S&P 500 · {} companies · weights {}",
                universe.constituents.len(),
                universe.source_as_of.as_deref().unwrap_or("(date unknown)").to_lowercase()
            ));
            if self.market.loading {
                ui.spinner();
                ui.weak("Loading quotes…");
            }
        });
        ui.add_space(4.0);

        let empty = Default::default();
        let market = self.market.value.as_ref().unwrap_or(&empty);
        let mut actions = Vec::new();

        egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
            for (sector, sector_weight) in universe.sectors_by_weight() {
                let mut rows: Vec<Row> = universe
                    .constituents
                    .iter()
                    .filter(|c| c.sector == sector)
                    .map(|c| {
                        let held = self.held.get(&c.symbol);
                        let dnt = self.dnt.contains(&c.symbol);
                        let (selected, selectable) = match self.mode {
                            Mode::Create => (self.basket.contains(&c.symbol), !dnt),
                            Mode::Rebalance => (
                                self.discards.contains(&c.symbol),
                                !dnt && held.is_some_and(Held::is_losing),
                            ),
                            Mode::Browse | Mode::Review | Mode::Execute => (false, false),
                        };
                        Row { c, md: market.get(&c.symbol), held, dnt, selected, selectable }
                    })
                    .collect();
                let sort = self.sort.get(&sector).copied().unwrap_or_default();
                rows.sort_by(|a, b| compare(a, b, sort));

                let n_held = rows.iter().filter(|r| r.held.is_some()).count();
                let mut title = format!("{sector}   {:.1}%   {} stocks", sector_weight * 100.0, rows.len());
                if n_held > 0 {
                    title.push_str(&format!("   · {n_held} held"));
                }

                egui::CollapsingHeader::new(RichText::new(title).strong())
                    .id_salt(&sector)
                    .show(ui, |ui| sector_table(ui, &sector, &rows, sort, self.mode, &mut actions));
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
    let select_label = match mode {
        Mode::Create => Some("Basket"),
        Mode::Rebalance => Some("Discard"),
        Mode::Browse | Mode::Review | Mode::Execute => None,
    };
    let mut table = TableBuilder::new(ui)
        .id_salt(sector)
        .striped(true)
        .vscroll(false)
        .resizable(true)
        .cell_layout(egui::Layout::left_to_right(egui::Align::Center));
    if select_label.is_some() {
        table = table.column(Column::exact(52.0));
    }
    table = table.column(Column::exact(28.0));
    for (col, _, _) in COLUMNS {
        table = table.column(match col {
            Col::Name => Column::initial(200.0).clip(true),
            Col::Symbol => Column::initial(64.0),
            _ => Column::initial(78.0),
        });
    }

    table
        .header(22.0, |mut header| {
            if let Some(label) = select_label {
                header.col(|ui| {
                    ui.strong(label);
                });
            }
            header.col(|ui| {
                ui.label("DNT").on_hover_text("Do not transact");
            });
            for (col, label, hover) in COLUMNS {
                header.col(|ui| {
                    let arrow = match (sort.col == *col, sort.ascending) {
                        (true, true) => " ▲",
                        (true, false) => " ▼",
                        _ => "",
                    };
                    let mut resp = ui.add(egui::Button::new(RichText::new(format!("{label}{arrow}")).strong()).frame(false));
                    if !hover.is_empty() {
                        resp = resp.on_hover_text(*hover);
                    }
                    if resp.clicked() {
                        actions.push(Action::Sort(sector.to_string(), *col));
                    }
                });
            }
        })
        .body(|mut body| {
            for r in rows {
                body.row(20.0, |mut row| {
                    if select_label.is_some() {
                        row.col(|ui| {
                            paint_tint(ui, r.held);
                            let mut on = r.selected;
                            let resp = ui.add_enabled(r.selectable, egui::Checkbox::without_text(&mut on));
                            let resp = if mode == Mode::Rebalance {
                                resp.on_disabled_hover_text(DISCARD_HINT)
                            } else {
                                resp.on_disabled_hover_text("On the do-not-transact list")
                            };
                            if resp.changed() {
                                actions.push(Action::Select(r.c.symbol.clone(), on));
                            }
                        });
                    }
                    row.col(|ui| {
                        paint_tint(ui, r.held);
                        let mut on = r.dnt;
                        if ui.checkbox(&mut on, "").on_hover_text("Do not transact").changed() {
                            actions.push(Action::SetDnt(r.c.symbol.clone(), on));
                        }
                    });
                    for (col, _, _) in COLUMNS {
                        row.col(|ui| {
                            paint_tint(ui, r.held);
                            cell(ui, r, *col);
                        });
                    }
                });
            }
        });
}

fn paint_tint(ui: &mut egui::Ui, held: Option<&Held>) {
    if let Some(t) = row_tint(ui, held) {
        ui.painter().rect_filled(ui.max_rect(), 0.0, t);
    }
}

fn cell(ui: &mut egui::Ui, r: &Row, col: Col) {
    let md = r.md;
    let text = match col {
        Col::Symbol => {
            let resp = ui.label(RichText::new(&r.c.symbol).monospace().strong());
            if !r.c.merged.is_empty() {
                resp.on_hover_text(format!("Includes {}", r.c.merged.join(", ")));
            }
            return;
        }
        Col::Name => {
            ui.label(r.name()).on_hover_text(r.name());
            return;
        }
        Col::Return => {
            if let Some(h) = r.held {
                let v = h.total_return();
                let color = gain_color(ui, v);
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.label(RichText::new(money(v)).color(color)).on_hover_text(format!(
                        "Value {}  +  dividends {}  −  cost {}",
                        money(h.market_value),
                        money(h.dividends),
                        money(h.cost)
                    ));
                });
            }
            return;
        }
        Col::Change => {
            if let Some(v) = md.and_then(|m| m.net_percent_change) {
                let color = gain_color(ui, v);
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.label(RichText::new(format!("{v:+.2}%")).color(color));
                });
            }
            return;
        }
        Col::Weight => format!("{:.3}%", r.c.weight * 100.0),
        Col::Price => opt(r.num(col), |v| format!("{v:.2}")),
        Col::Low52 | Col::High52 | Col::Eps => opt(r.num(col), |v| format!("{v:.2}")),
        Col::Pe => opt(r.num(col), |v| format!("{v:.1}")),
        Col::Yield => opt(r.num(col), |v| format!("{v:.2}%")),
        Col::Freq => freq_label(md.and_then(|m| m.div_freq)).to_string(),
        Col::Volume => opt(r.num(col), |v| group_thousands(&format!("{v:.0}"))),
        Col::Held => r.held.map(|h| fmt_qty(h.quantity)).unwrap_or_default(),
    };
    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
        ui.label(text);
    });
}

/// Share counts: whole numbers plainly, fractions to 4 places, no trailing zeros.
pub fn fmt_qty(q: f64) -> String {
    let s = format!("{q:.4}");
    s.trim_end_matches('0').trim_end_matches('.').to_string()
}
