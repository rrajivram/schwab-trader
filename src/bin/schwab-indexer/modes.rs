//! Create / Rebalance side panels and the final Review screen.

use std::cmp::Ordering;
use std::collections::{HashMap, HashSet};

use eframe::egui;
use egui_extras::{Column, TableBuilder};
use schwab::api::MarketData;
use schwab::basket::{self, Pick};
use schwab::execution::PlannedOrder;
use schwab::orders::Side;

use crate::app::{money, ticker_and_name, ticker_with_name, IndexerApp};
use crate::home::fmt_qty;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Browse,
    Create,
    Rebalance,
    Review,
    /// Placing (or previewing) orders.
    Execute,
}

pub struct Review {
    pub rebalance: bool,
    pub lines: Vec<Line>,
    /// Dollars to invest; clamped to `cap`.
    pub amount: f64,
    pub cap: f64,
    /// Rebalance only: positions sold in full, with their market value.
    pub sells: Vec<(String, f64)>,
    pub add_input: String,
    pub message: Option<String>,
    /// (index into `REVIEW_COLUMNS`, ascending); None keeps the pick order.
    pub sort: Option<(usize, bool)>,
    /// Confirm dialog open, holding its "preview only" checkbox state.
    pub confirm: Option<bool>,
}

/// Review table headers; all but the trailing remove-button column sort.
const REVIEW_COLUMNS: [&str; 10] = ["Symbol", "Company", "Sector", "Div Yld", "Price", "Weight", "Share", "Amount", "Shares", ""];
/// Columns 0..TEXT_COLUMNS sort alphabetically, the rest numerically.
const TEXT_COLUMNS: usize = 3;

pub struct Line {
    pub symbol: String,
    pub sector: String,
    /// Raw S&P 500 weight, for "reset to index weights".
    pub index_weight: f64,
    /// User-editable relative weight in percent; rescaled to 100% when sizing.
    pub weight_pct: f64,
}

impl IndexerApp {
    /// Auto's ranking metric: EPS.
    fn eps_scores(&self) -> HashMap<String, f64> {
        self.market
            .value
            .as_ref()
            .map(|m| m.iter().filter_map(|(s, md)| Some((s.clone(), md.eps?))).collect())
            .unwrap_or_default()
    }

    fn cash(&self) -> f64 {
        self.account.value.as_ref().map(|a| a.cash_balance).unwrap_or(0.0)
    }

    fn index_weight(&self, symbol: &str) -> Option<(f64, String)> {
        let u = self.universe.value.as_ref()?;
        let c = u.constituents.iter().find(|c| c.symbol == symbol)?;
        Some((c.weight, c.sector.clone()))
    }

    /// Stocks Auto never picks: do-not-transact plus everything already held.
    fn auto_exclusions(&self) -> HashSet<String> {
        self.dnt.iter().cloned().chain(self.held.keys().cloned()).collect()
    }

    /// Stocks failing the Max P/E filter: P/E above the limit, or no
    /// positive P/E at all (unknown, or losses make it meaningless).
    fn pe_exclusions(&self) -> HashSet<String> {
        if !self.max_pe_enabled {
            return HashSet::new();
        }
        let (Some(u), Some(market)) = (&self.universe.value, &self.market.value) else { return HashSet::new() };
        u.constituents
            .iter()
            .filter(|c| {
                let pe = market.get(&c.symbol).and_then(|m| m.pe_ratio);
                !pe.is_some_and(|pe| pe > 0.0 && pe <= self.max_pe)
            })
            .map(|c| c.symbol.clone())
            .collect()
    }

    fn discard_value(&self) -> f64 {
        self.discards.iter().filter_map(|s| self.held.get(s)).map(|h| h.market_value).sum()
    }

    /// Current rebalance replacements, recomputed from the discard list.
    pub(crate) fn current_replacements(&self) -> Vec<Pick> {
        let Some(u) = &self.universe.value else { return Vec::new() };
        let sectors: Vec<String> = self.discards.iter().filter_map(|s| self.index_weight(s).map(|(_, sec)| sec)).collect();
        basket::replacements(u, &self.eps_scores(), &self.auto_exclusions(), &sectors)
    }

    pub(crate) fn create_panel(&mut self, ui: &mut egui::Ui) {
        let mut remove = None;
        let mut auto = false;
        let mut review = false;
        let names = self.names(&self.basket);
        let pe_passing = self.universe.value.as_ref().map_or(0, |u| u.constituents.len()) - self.pe_exclusions().len();
        egui::Panel::right("create_panel").default_size(260.0).show(ui, |ui| {
            ui.heading("Create basket");
            ui.label("Tick “Basket” on any row, or fill automatically.");
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                ui.label("Auto size");
                ui.add(egui::DragValue::new(&mut self.auto_size).range(1..=500).suffix(" stocks"));
            });
            ui.horizontal(|ui| {
                ui.checkbox(&mut self.max_pe_enabled, "Max P/E")
                    .on_hover_text("Only pick stocks with a positive P/E at or below this. Stocks with no P/E (e.g. losses) are skipped.");
                ui.add_enabled(self.max_pe_enabled, egui::DragValue::new(&mut self.max_pe).range(1.0..=500.0).speed(0.5).max_decimals(1));
            });
            if self.max_pe_enabled {
                ui.weak(format!("{pe_passing} S&P 500 stocks pass (before other exclusions)"));
            }
            let ready = self.market.value.is_some();
            auto = ui
                .add_enabled(ready, egui::Button::new("Auto fill"))
                .on_hover_text(
                    "Skips do-not-transact and stocks you hold, then takes the highest EPS \
                     from each sector (heaviest sector first), then the 2nd highest, …",
                )
                .on_disabled_hover_text("Waiting for quotes")
                .clicked();
            if let Some(note) = &self.auto_note {
                ui.colored_label(ui.visuals().warn_fg_color, note);
            }
            ui.separator();
            ui.horizontal(|ui| {
                ui.strong(format!("Basket ({})", self.basket.len()));
                if !self.basket.is_empty() && ui.small_button("Clear").clicked() {
                    self.basket.clear();
                }
            });
            egui::ScrollArea::vertical().max_height(ui.available_height() - 40.0).show(ui, |ui| {
                for sym in &self.basket {
                    ui.horizontal(|ui| {
                        if ui.small_button("✕").clicked() {
                            remove = Some(sym.clone());
                        }
                        ticker_with_name(ui, sym, &names[sym]);
                    });
                }
            });
            ui.separator();
            review = ui.add_enabled(!self.basket.is_empty(), egui::Button::new("Review →")).clicked();
        });

        if let Some(sym) = remove {
            self.basket.retain(|s| *s != sym);
        }
        if auto {
            if let Some(u) = &self.universe.value {
                let mut exclude = self.auto_exclusions();
                exclude.extend(self.pe_exclusions());
                let picks = basket::auto_basket(u, &self.eps_scores(), &exclude, self.auto_size);
                self.auto_note = (picks.len() < self.auto_size)
                    .then(|| format!("Only {} stocks qualify — basket has {}.", picks.len(), picks.len()));
                self.basket = picks.into_iter().map(|p| p.symbol).collect();
            }
        }
        if review {
            self.start_review(false);
        }
    }

    pub(crate) fn rebalance_panel(&mut self, ui: &mut egui::Ui) {
        let mut remove = None;
        let mut review = false;
        let replacements = self.current_replacements();
        let names = self.names(self.discards.iter().chain(replacements.iter().map(|p| &p.symbol)));
        egui::Panel::right("rebalance_panel").default_size(300.0).show(ui, |ui| {
            ui.heading("Rebalance");
            ui.label("Tick “Discard” on held stocks that are below purchase price (red rows).");
            ui.add_space(6.0);
            ui.strong(format!("Discard ({}) · {}", self.discards.len(), money(self.discard_value())));
            for sym in &self.discards {
                ui.horizontal(|ui| {
                    if ui.small_button("✕").clicked() {
                        remove = Some(sym.clone());
                    }
                    if let Some(h) = self.held.get(sym) {
                        ui.label(money(h.market_value));
                    }
                    ticker_with_name(ui, sym, &names[sym]);
                });
            }
            ui.separator();
            ui.strong("Replacements");
            ui.weak("Each is the highest-EPS stock in the next sector after the discard's.");
            if replacements.is_empty() {
                ui.weak("—");
            }
            for (discard, pick) in self.discards.iter().zip(&replacements) {
                ui.label(format!(
                    "{} → {}",
                    ticker_and_name(discard, &names[discard]),
                    ticker_and_name(&pick.symbol, &names[&pick.symbol])
                ));
                let eps = pick.score.map(|e| format!("{e:.2}")).unwrap_or("—".into());
                ui.weak(format!("      {}, EPS {eps}", pick.sector));
            }
            ui.separator();
            review = ui.add_enabled(!replacements.is_empty(), egui::Button::new("Review →")).clicked();
        });

        if let Some(sym) = remove {
            self.discards.retain(|s| *s != sym);
        }
        if review {
            self.start_review(true);
        }
    }

    fn start_review(&mut self, rebalance: bool) {
        let (symbols, sells, cap, default_amount) = if rebalance {
            let sells: Vec<(String, f64)> = self
                .discards
                .iter()
                .filter_map(|s| Some((s.clone(), self.held.get(s)?.market_value)))
                .collect();
            let proceeds: f64 = sells.iter().map(|(_, v)| v).sum();
            let symbols = self.current_replacements().into_iter().map(|p| p.symbol).collect();
            (symbols, sells, self.cash() + proceeds, proceeds)
        } else {
            (self.basket.clone(), Vec::new(), self.cash(), self.cash())
        };

        let mut lines: Vec<Line> = symbols
            .iter()
            .filter_map(|s| {
                let (w, sector) = self.index_weight(s)?;
                Some(Line { symbol: s.clone(), sector, index_weight: w, weight_pct: 0.0 })
            })
            .collect();
        reset_weights(&mut lines);

        self.review = Some(Review {
            rebalance,
            lines,
            amount: default_amount.min(cap).max(0.0),
            cap: cap.max(0.0),
            sells,
            add_input: String::new(),
            message: None,
            sort: None,
            confirm: None,
        });
        self.mode = Mode::Review;
    }

    pub(crate) fn review_ui(&mut self, ui: &mut egui::Ui) {
        let Some(mut review) = self.review.take() else {
            self.mode = Mode::Browse;
            return;
        };
        let market = self.market.value.clone().unwrap_or_default();
        let mut back = false;
        let mut add = None;
        let mut planned_buys = Vec::new();
        let mut skipped = Vec::new();

        egui::CentralPanel::default().show(ui, |ui| {
            ui.horizontal(|ui| {
                back = ui.button("← Back").clicked();
                ui.heading(if review.rebalance { "Review rebalance" } else { "Review new basket" });
            });
            ui.add_space(6.0);

            if review.rebalance {
                let sold: Vec<String> = review
                    .sells
                    .iter()
                    .map(|(s, v)| {
                        let name = market.get(s).and_then(|m| m.description.as_deref()).unwrap_or("");
                        format!("{} {}", ticker_and_name(s, name), money(*v))
                    })
                    .collect();
                ui.label(format!("Sell in full: {}", sold.join(", ")));
            }

            ui.horizontal(|ui| {
                ui.label("Investment amount");
                ui.add(
                    egui::DragValue::new(&mut review.amount)
                        .range(0.0..=review.cap)
                        .speed(10.0)
                        .prefix("$")
                        .fixed_decimals(2),
                );
                let cap_note = if review.rebalance { "cash + sale proceeds" } else { "available cash" };
                ui.weak(format!("max {} ({cap_note})", money(review.cap)));
            });
            ui.add_space(6.0);

            // Re-sort each frame so edits and additions land in place, but not
            // mid-edit — rows jumping under the cursor while dragging a weight
            // would be unusable.
            let editing = ui.ctx().egui_is_using_pointer() || ui.ctx().memory(|m| m.focused().is_some());
            if let (Some(sort), false) = (review.sort, editing) {
                sort_lines(&mut review.lines, &market, sort);
            }

            let weights: Vec<f64> = review.lines.iter().map(|l| l.weight_pct).collect();
            let prices: Vec<Option<f64>> = review.lines.iter().map(|l| market.get(&l.symbol).and_then(|m| m.price)).collect();
            let alloc = basket::allocate(review.amount, &weights, &prices);
            let shares = basket::normalized(&weights);
            let mut remove = None;
            let mut clicked_col = None;
            let sort = review.sort;

            egui::ScrollArea::vertical().max_height(ui.available_height() - 90.0).show(ui, |ui| {
                TableBuilder::new(ui)
                    .id_salt("review_table")
                    .striped(true)
                    .vscroll(false)
                    .cell_layout(egui::Layout::left_to_right(egui::Align::Center))
                    .column(Column::exact(64.0))
                    .column(Column::initial(220.0).clip(true))
                    .column(Column::initial(170.0))
                    .columns(Column::initial(84.0), 6)
                    .column(Column::exact(28.0))
                    .header(22.0, |mut h| {
                        for (i, label) in REVIEW_COLUMNS.iter().enumerate() {
                            h.col(|ui| {
                                if label.is_empty() {
                                    return;
                                }
                                let arrow = match sort {
                                    Some((c, true)) if c == i => " ▲",
                                    Some((c, false)) if c == i => " ▼",
                                    _ => "",
                                };
                                let text = egui::RichText::new(format!("{label}{arrow}")).strong();
                                if ui.add(egui::Button::new(text).frame(false)).clicked() {
                                    clicked_col = Some(i);
                                }
                            });
                        }
                    })
                    .body(|mut body| {
                        for (i, line) in review.lines.iter_mut().enumerate() {
                            let md = market.get(&line.symbol);
                            body.row(22.0, |mut row| {
                                row.col(|ui| {
                                    ui.monospace(&line.symbol);
                                });
                                row.col(|ui| {
                                    let name = md.and_then(|m| m.description.as_deref()).unwrap_or("");
                                    ui.label(name).on_hover_text(name);
                                });
                                row.col(|ui| {
                                    ui.label(&line.sector);
                                });
                                row.col(|ui| {
                                    ui.label(md.and_then(|m| m.div_yield).map(|y| format!("{y:.2}%")).unwrap_or("—".into()));
                                });
                                row.col(|ui| {
                                    ui.label(prices[i].map(|p| format!("{p:.2}")).unwrap_or("—".into()));
                                });
                                row.col(|ui| {
                                    ui.add(egui::DragValue::new(&mut line.weight_pct).range(0.0..=100.0).speed(0.1).suffix("%").max_decimals(2))
                                        .on_hover_text("Relative weight — rescaled so the basket totals 100%");
                                });
                                row.col(|ui| {
                                    ui.label(format!("{:.2}%", shares[i] * 100.0));
                                });
                                row.col(|ui| {
                                    ui.label(money(alloc[i].dollars));
                                });
                                row.col(|ui| {
                                    if prices[i].is_some() {
                                        ui.label(fmt_qty(alloc[i].quantity));
                                    } else {
                                        ui.weak("no price");
                                    }
                                });
                                row.col(|ui| {
                                    if ui.small_button("✕").on_hover_text("Remove").clicked() {
                                        remove = Some(i);
                                    }
                                });
                            });
                        }
                    });
            });

            let spent: f64 = alloc
                .iter()
                .zip(&prices)
                .filter_map(|(a, p)| Some(a.quantity * (*p)?))
                .sum();
            for ((line, a), price) in review.lines.iter().zip(&alloc).zip(&prices) {
                match price {
                    Some(p) if a.quantity > 0.0 => planned_buys.push(PlannedOrder {
                        side: Side::Buy,
                        symbol: line.symbol.clone(),
                        schwab_symbol: line.symbol.replace('.', "/"),
                        quantity: a.quantity,
                        est_price: *p,
                    }),
                    _ => skipped.push(line.symbol.clone()),
                }
            }
            ui.separator();
            ui.horizontal(|ui| {
                ui.strong(format!("{} stocks · buys total {}", review.lines.len(), money(spent)));
                ui.weak(format!("· {} left over from rounding shares down", money(review.amount - spent)));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let can_place = !planned_buys.is_empty() || (review.rebalance && !review.sells.is_empty());
                    let place = egui::Button::new(egui::RichText::new("Place orders…").strong());
                    if ui.add_enabled(can_place, place).clicked() {
                        review.confirm = Some(true);
                    }
                });
            });
            ui.horizontal(|ui| {
                let resp = ui.add(egui::TextEdit::singleline(&mut review.add_input).hint_text("Add symbol").desired_width(90.0));
                let enter = resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                if (ui.button("Add").clicked() || enter) && !review.add_input.trim().is_empty() {
                    add = Some(review.add_input.trim().to_uppercase());
                }
                if ui.button("Reset to index weights").clicked() {
                    reset_weights(&mut review.lines);
                }
            });
            if let Some(msg) = &review.message {
                ui.colored_label(ui.visuals().warn_fg_color, msg);
            }

            if let Some(i) = remove {
                review.lines.remove(i);
            }
            if let Some(col) = clicked_col {
                review.sort = Some(match review.sort {
                    Some((c, asc)) if c == col => (col, !asc),
                    // Text sorts A→Z first; numbers biggest first.
                    _ => (col, col < TEXT_COLUMNS),
                });
            }
        });

        if let Some(sym) = add {
            review.message = self.add_line(&mut review, &sym).err();
            if review.message.is_none() {
                review.add_input.clear();
            }
        }

        let launch = self.confirm_modal(ui, &mut review, planned_buys, &skipped, &market);

        if back {
            self.mode = if review.rebalance { Mode::Rebalance } else { Mode::Create };
        } else {
            self.review = Some(review);
        }
        if let Some(plan) = launch {
            self.start_execution(plan);
        }
    }

    fn add_line(&self, review: &mut Review, symbol: &str) -> Result<(), String> {
        let symbol = schwab::universe::universe_symbol(symbol);
        if review.lines.iter().any(|l| l.symbol == symbol) {
            return Err(format!("{symbol} is already in the basket"));
        }
        if self.dnt.contains(&symbol) {
            return Err(format!("{symbol} is on the do-not-transact list"));
        }
        let (index_weight, sector) = self.index_weight(&symbol).ok_or(format!("{symbol} is not in the S&P 500"))?;
        // Keep the newcomer in index proportion to the lines already there.
        let pct: f64 = review.lines.iter().map(|l| l.weight_pct).sum();
        let idx: f64 = review.lines.iter().map(|l| l.index_weight).sum();
        let weight_pct = if idx > 0.0 { index_weight * pct / idx } else { 100.0 };
        review.lines.push(Line { symbol, sector, index_weight, weight_pct });
        Ok(())
    }
}

fn sort_lines(lines: &mut [Line], market: &HashMap<String, MarketData>, (col, ascending): (usize, bool)) {
    let name = |l: &Line| market.get(&l.symbol).and_then(|m| m.description.clone()).unwrap_or_default();
    let price = |l: &Line| market.get(&l.symbol).and_then(|m| m.price);
    // Share and Amount are proportional to Weight, and Shares to Weight/Price,
    // so no need to compute the allocation to order by them.
    let num = |l: &Line| -> Option<f64> {
        match REVIEW_COLUMNS[col] {
            "Div Yld" => market.get(&l.symbol).and_then(|m| m.div_yield),
            "Price" => price(l),
            "Weight" | "Share" | "Amount" => Some(l.weight_pct),
            "Shares" => price(l).filter(|p| *p > 0.0).map(|p| l.weight_pct / p),
            _ => None,
        }
    };
    let dir = |o: Ordering| if ascending { o } else { o.reverse() };
    lines.sort_by(|a, b| {
        match REVIEW_COLUMNS[col] {
            "Symbol" => dir(a.symbol.cmp(&b.symbol)),
            "Company" => dir(name(a).cmp(&name(b))),
            "Sector" => dir(a.sector.cmp(&b.sector)),
            _ => match (num(a), num(b)) {
                (Some(x), Some(y)) => dir(x.total_cmp(&y)),
                // Missing values last in either direction.
                (Some(_), None) => Ordering::Less,
                (None, Some(_)) => Ordering::Greater,
                (None, None) => Ordering::Equal,
            },
        }
        .then_with(|| a.symbol.cmp(&b.symbol))
    });
}

fn reset_weights(lines: &mut [Line]) {
    let w: Vec<f64> = lines.iter().map(|l| l.index_weight).collect();
    for (line, n) in lines.iter_mut().zip(basket::normalized(&w)) {
        line.weight_pct = n * 100.0;
    }
}

/// Short explanation shown on a disabled Discard box.
pub const DISCARD_HINT: &str = "Only held stocks below purchase price (including dividends) can be discarded";
