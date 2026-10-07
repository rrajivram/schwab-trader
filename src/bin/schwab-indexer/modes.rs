//! Create / Rebalance side panels and the final Review screen.

use std::cmp::Ordering;
use std::collections::{HashMap, HashSet};

use eframe::egui::{self, RichText};
use egui_extras::{Column, TableBuilder};
use schwab::alphavantage::Overview;
use schwab::api::MarketData;
use schwab::basket::{self, Pick};
use schwab::execution::PlannedOrder;
use schwab::orders::Side;
use schwab::risk::Fit;

use crate::app::{money, IndexerApp};
use crate::theme::{self, pal, Tone};
use crate::home::fmt_qty;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Browse,
    Create,
    Rebalance,
    Review,
    /// Placing (or previewing) orders.
    Execute,
    /// Bond maturities and coupons.
    Bonds,
    /// The local divestiture-plan page in a web view.
    Plan,
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
const REVIEW_COLUMNS: [&str; 17] = [
    "Symbol", "Company", "Sector", "Div Yld", "P/E", "Fwd P/E", "Beta", "Alpha", "Target", "Upside", "Analysts", "Price", "Weight", "Share",
    "Amount", "Shares", "",
];
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

    fn side_frame(ui: &egui::Ui) -> egui::Frame {
        let p = pal(ui);
        egui::Frame::new().fill(p.surface).stroke(egui::Stroke::new(1.0, p.border)).inner_margin(egui::Margin::same(16))
    }

    pub(crate) fn create_panel(&mut self, ui: &mut egui::Ui) {
        let p = pal(ui);
        let mut remove = None;
        let mut auto = false;
        let mut review = false;
        let names = self.names(&self.basket);
        let pe_passing = self.universe.value.as_ref().map_or(0, |u| u.constituents.len()) - self.pe_exclusions().len();
        egui::Panel::right("create_panel").frame(Self::side_frame(ui)).default_size(300.0).show(ui, |ui| {
            ui.label(RichText::new("New basket").font(theme::sans_semibold(theme::TITLE)));
            ui.label(RichText::new("Add stocks from the list, or let Auto fill choose them.").color(p.muted));
            ui.add_space(10.0);

            ui.label(theme::eyebrow(ui, "Auto fill"));
            egui::Grid::new("auto_grid").num_columns(2).spacing([10.0, 8.0]).show(ui, |ui| {
                ui.label("Basket size");
                ui.add(egui::DragValue::new(&mut self.auto_size).range(1..=500).suffix(" stocks"));
                ui.end_row();
                ui.checkbox(&mut self.max_pe_enabled, "Max P/E")
                    .on_hover_text("Only pick stocks with a positive P/E at or below this. Stocks with no P/E (e.g. losses) are skipped.");
                ui.add_enabled(self.max_pe_enabled, egui::DragValue::new(&mut self.max_pe).range(1.0..=500.0).speed(0.5).max_decimals(1));
                ui.end_row();
            });
            if self.max_pe_enabled {
                ui.label(RichText::new(format!("{pe_passing} of the S&P 500 pass this limit")).color(p.muted).font(theme::sans(theme::SMALL)));
            }
            let ready = self.market.value.is_some();
            auto = ui
                .add_enabled_ui(ready, |ui| ui.add_sized([ui.available_width(), 32.0], theme::primary(ui, "Auto fill")))
                .inner
                .on_hover_text(
                    "Skips blocked stocks and ones you hold, then takes the highest-EPS stock \
                     from each sector (heaviest sector first), then the 2nd highest, and so on.",
                )
                .on_disabled_hover_text("Waiting for quotes")
                .clicked();
            if let Some(note) = &self.auto_note {
                ui.label(RichText::new(note).color(p.warn));
            }

            ui.add_space(8.0);
            ui.separator();
            ui.horizontal(|ui| {
                ui.label(theme::eyebrow(ui, &format!("Basket · {}", self.basket.len())));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if !self.basket.is_empty() && ui.small_button("Clear all").clicked() {
                        self.basket.clear();
                    }
                });
            });
            egui::ScrollArea::vertical().max_height(ui.available_height() - 48.0).show(ui, |ui| {
                if self.basket.is_empty() {
                    ui.label(RichText::new("Empty. Click Add on any row, or use Auto fill.").color(p.muted));
                }
                for sym in &self.basket {
                    list_row(ui, sym, &names[sym], None, || remove = Some(sym.clone()));
                }
            });
            ui.separator();
            review = ui
                .add_enabled_ui(!self.basket.is_empty(), |ui| ui.add_sized([ui.available_width(), 32.0], theme::primary(ui, "Review basket →")))
                .inner
                .clicked();
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
                    .then(|| format!("Only {} stocks qualify, so the basket has {}.", picks.len(), picks.len()));
                self.basket = picks.into_iter().map(|p| p.symbol).collect();
            }
        }
        if review {
            self.start_review(false);
        }
    }

    pub(crate) fn rebalance_panel(&mut self, ui: &mut egui::Ui) {
        let p = pal(ui);
        let mut remove = None;
        let mut review = false;
        let replacements = self.current_replacements();
        let names = self.names(self.discards.iter().chain(replacements.iter().map(|p| &p.symbol)));
        egui::Panel::right("rebalance_panel").frame(Self::side_frame(ui)).default_size(320.0).show(ui, |ui| {
            ui.label(RichText::new("Rebalance").font(theme::sans_semibold(theme::TITLE)));
            ui.label(RichText::new("Click Discard on holdings that are below cost (red stripe). Each is sold and replaced.").color(p.muted));
            ui.add_space(10.0);

            ui.label(theme::eyebrow(ui, &format!("Selling · {} · {}", self.discards.len(), money(self.discard_value()))));
            if self.discards.is_empty() {
                ui.label(RichText::new("Nothing to sell yet.").color(p.muted));
            }
            for sym in &self.discards {
                let value = self.held.get(sym).map(|h| money(h.market_value));
                list_row(ui, sym, &names[sym], value.as_deref(), || remove = Some(sym.clone()));
            }

            ui.add_space(8.0);
            ui.separator();
            ui.label(theme::eyebrow(ui, "Replacements"));
            ui.label(RichText::new("The highest-EPS stock in the next sector after each discard's.").color(p.muted).font(theme::sans(theme::SMALL)));
            if replacements.is_empty() {
                ui.label(RichText::new("—").color(p.muted));
            }
            for (discard, pick) in self.discards.iter().zip(&replacements) {
                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    ui.label(RichText::new(&pick.symbol).font(theme::mono_semibold(theme::BODY)));
                    ui.add(egui::Label::new(RichText::new(&names[&pick.symbol]).color(p.ink)).truncate());
                });
                let eps = pick.score.map(|e| format!("{e:.2}")).unwrap_or("—".into());
                ui.label(RichText::new(format!("replaces {discard} · {} · EPS {eps}", pick.sector)).color(p.muted).font(theme::sans(theme::SMALL)));
            }
            ui.add_space(8.0);
            ui.separator();
            review = ui
                .add_enabled_ui(!replacements.is_empty(), |ui| ui.add_sized([ui.available_width(), 32.0], theme::primary(ui, "Review rebalance →")))
                .inner
                .clicked();
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
        let symbols = self.review.as_ref().map(|r| r.lines.iter().map(|l| l.symbol.clone()).collect()).unwrap_or_default();
        self.fetch_overviews(symbols);
    }

    pub(crate) fn review_ui(&mut self, ui: &mut egui::Ui) {
        let Some(mut review) = self.review.take() else {
            self.mode = Mode::Browse;
            return;
        };
        let p = pal(ui);
        let market = self.market.value.clone().unwrap_or_default();
        let symbols: Vec<String> = review.lines.iter().map(|l| l.symbol.clone()).collect();
        let names = self.names(&symbols);
        let overviews: HashMap<String, Overview> =
            symbols.iter().filter_map(|s| Some((s.clone(), self.overviews.get(s)?.clone()))).collect();
        let fits: HashMap<String, Fit> = symbols.iter().filter_map(|s| Some((s.clone(), *self.fits.get(s)?))).collect();
        let (av_running, av_used, av_notes) = (self.av_running, self.av_used_today, self.av_notes.clone());
        let mut back = false;
        let mut add = None;
        let mut planned_buys = Vec::new();
        let mut skipped = Vec::new();

        let frame = egui::Frame::new().fill(p.bg).inner_margin(egui::Margin::symmetric(22, 16));
        egui::CentralPanel::default().frame(frame).show(ui, |ui| {
            // Re-sort each frame so edits and additions land in place, but not
            // mid-edit — rows jumping under the cursor while dragging a weight
            // would be unusable.
            let editing = ui.ctx().egui_is_using_pointer() || ui.ctx().memory(|m| m.focused().is_some());
            if let (Some(sort), false) = (review.sort, editing) {
                sort_lines(&mut review.lines, &market, &overviews, &fits, &names, sort);
            }
            let weights: Vec<f64> = review.lines.iter().map(|l| l.weight_pct).collect();
            let prices: Vec<Option<f64>> = review.lines.iter().map(|l| market.get(&l.symbol).and_then(|m| m.price)).collect();
            let alloc = basket::allocate(review.amount, &weights, &prices);
            let shares = basket::normalized(&weights);
            let spent: f64 = alloc.iter().zip(&prices).filter_map(|(a, p)| Some(a.quantity * (*p)?)).sum();
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
            let can_place = !planned_buys.is_empty() || (review.rebalance && !review.sells.is_empty());

            // Title row.
            ui.horizontal(|ui| {
                back = ui.button("← Back").clicked();
                ui.add_space(6.0);
                ui.label(
                    RichText::new(if review.rebalance { "Review rebalance" } else { "Review new basket" })
                        .font(theme::sans_semibold(theme::HEADING)),
                );
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.add_enabled(can_place, theme::primary(ui, "Place orders…").min_size(egui::Vec2::new(140.0, 32.0))).clicked() {
                        review.confirm = Some(true);
                    }
                });
            });
            ui.add_space(10.0);

            // Summary.
            theme::card(ui).inner_margin(egui::Margin::symmetric(18, 14)).show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 36.0;
                    ui.vertical(|ui| {
                        ui.spacing_mut().item_spacing.y = 2.0;
                        ui.label(theme::eyebrow(ui, "Investment amount"));
                        ui.add(
                            egui::DragValue::new(&mut review.amount)
                                .range(0.0..=review.cap)
                                .speed(10.0)
                                .prefix("$")
                                .fixed_decimals(2)
                                .custom_formatter(|v, _| money(v).trim_start_matches('$').to_string()),
                        )
                        .on_hover_text("Drag or click to type");
                        let cap_note = if review.rebalance { "cash + sale proceeds" } else { "available cash" };
                        ui.label(RichText::new(format!("max {} · {cap_note}", money(review.cap))).color(p.muted).font(theme::sans(theme::SMALL)));
                    });
                    theme::stat(ui, "Stocks", &review.lines.len().to_string(), None);
                    theme::stat(ui, "Buys total", &money(spent), None);
                    theme::stat(ui, "Left over", &money(review.amount - spent), Some(p.muted));
                    let beta = schwab::risk::weighted_beta(
                        review.lines.iter().zip(&shares).map(|(l, &s)| (s, market.get(&l.symbol).and_then(|m| m.beta))),
                    );
                    match beta {
                        Some(b) => theme::stat(ui, "Basket beta", &format!("{:.2}", b.beta), None).on_hover_text(format!(
                            "Weighted by each stock's share of the basket ({} of {} have a Schwab beta). \
                             1.0 moves with the S&P 500; above 1 swings more, below 1 less.",
                            b.count,
                            review.lines.len()
                        )),
                        None => theme::stat(ui, "Basket beta", "—", Some(p.muted)),
                    };
                    if review.rebalance {
                        let sold: f64 = review.sells.iter().map(|(_, v)| v).sum();
                        theme::stat(ui, "Selling", &money(sold), Some(p.loss));
                    }
                });
                if review.rebalance && !review.sells.is_empty() {
                    ui.add_space(8.0);
                    ui.horizontal_wrapped(|ui| {
                        ui.label(RichText::new("Sold in full first:").color(p.muted));
                        for (s, v) in &review.sells {
                            theme::tone_pill(ui, &format!("SELL {s} · {}", money(*v)), Tone::Loss);
                        }
                    });
                }
            });
            ui.add_space(10.0);

            ui.horizontal_wrapped(|ui| {
                ui.label(theme::eyebrow(ui, "Analyst data · Alpha Vantage"));
                let loaded = review.lines.iter().filter(|l| overviews.contains_key(&l.symbol)).count();
                ui.label(RichText::new(format!("{loaded} of {} loaded", review.lines.len())).color(p.muted));
                if av_running {
                    ui.spinner();
                }
                ui.label(
                    RichText::new(format!("· {av_used} of {} requests used today", schwab::alphavantage::DAILY_LIMIT))
                        .color(p.muted),
                );
                for n in &av_notes {
                    ui.label(RichText::new(n).color(p.warn));
                }
            });
            ui.add_space(4.0);

            let mut remove = None;
            let mut clicked_col = None;
            let sort = review.sort;
            theme::card(ui).inner_margin(egui::Margin::symmetric(14, 8)).show(ui, |ui| {
                egui::ScrollArea::both().max_height(ui.available_height() - 70.0).auto_shrink([false, true]).show(ui, |ui| {
                    TableBuilder::new(ui)
                        .id_salt("review_table")
                        .striped(true)
                        .vscroll(false)
                        .cell_layout(egui::Layout::left_to_right(egui::Align::Center))
                        .column(Column::exact(72.0))
                        .column(Column::initial(230.0).clip(true))
                        .column(Column::initial(170.0))
                        .columns(Column::initial(76.0), 7)
                        .column(Column::initial(100.0))
                        .columns(Column::initial(88.0), 5)
                        .column(Column::exact(80.0))
                        .header(26.0, |mut h| {
                            for (i, label) in REVIEW_COLUMNS.iter().enumerate() {
                                h.col(|ui| {
                                    if label.is_empty() {
                                        return;
                                    }
                                    let active = sort.and_then(|(c, asc)| (c == i).then_some(asc));
                                    let hint = match *label {
                                        "Weight" => "Your relative weight. Drag or type to change it.",
                                        "Share" => "Weight rescaled so the basket totals 100%",
                                        "Fwd P/E" => "Price ÷ analysts' expected earnings (Alpha Vantage)",
                                        "Beta" => "How much it moves with the S&P 500: 1.0 = in step, 2.0 = twice as much (Schwab)",
                                        "Alpha" => "Return per year beyond what beta explains (3 years weekly vs. SPY). Colored only when |t| ≥ 2.",
                                        "Target" => "Average analyst 12-month price target",
                                        "Upside" => "Target vs. current price",
                                        "Analysts" => "Share of analysts rating it Buy or Strong Buy",
                                        _ => "",
                                    };
                                    if theme::sort_header(ui, label, active, hint) {
                                        clicked_col = Some(i);
                                    }
                                });
                            }
                        })
                        .body(|mut body| {
                            for (i, line) in review.lines.iter_mut().enumerate() {
                                let md = market.get(&line.symbol);
                                let fig = |s: String| RichText::new(s).font(theme::mono(12.5)).color(p.ink);
                                body.row(28.0, |mut row| {
                                    row.col(|ui| {
                                        ui.label(RichText::new(&line.symbol).font(theme::mono_semibold(13.0)));
                                    });
                                    row.col(|ui| {
                                        let name = &names[&line.symbol];
                                        ui.label(name).on_hover_text(name);
                                    });
                                    row.col(|ui| {
                                        ui.label(RichText::new(&line.sector).color(p.muted));
                                    });
                                    row.col(|ui| {
                                        right(ui, fig(md.and_then(|m| m.div_yield).map(|y| format!("{y:.2}%")).unwrap_or("—".into())));
                                    });
                                    row.col(|ui| {
                                        right(ui, fig(md.and_then(|m| m.pe_ratio).map(|v| format!("{v:.1}")).unwrap_or("—".into())));
                                    });
                                    let ov = overviews.get(&line.symbol);
                                    // "…" while it may still arrive, "—" once fetched without a value.
                                    let blank = if ov.is_none() && av_running { "…" } else { "—" };
                                    row.col(|ui| {
                                        right(ui, fig(ov.and_then(|o| o.forward_pe).map(|v| format!("{v:.1}")).unwrap_or(blank.into())));
                                    });
                                    row.col(|ui| {
                                        right(ui, fig(md.and_then(|m| m.beta).map(|v| format!("{v:.2}")).unwrap_or("—".into())));
                                    });
                                    row.col(|ui| match fits.get(&line.symbol) {
                                        Some(f) => {
                                            let color = match (f.significant(), f.alpha >= 0.0) {
                                                (false, _) => p.muted,
                                                (true, true) => p.gain,
                                                (true, false) => p.loss,
                                            };
                                            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                                ui.label(RichText::new(format!("{:+.1}%", f.alpha * 100.0)).font(theme::mono(12.5)).color(color))
                                                    .on_hover_text(crate::home::fit_hint(f));
                                            });
                                        }
                                        None => right(ui, fig("—".into())),
                                    });
                                    row.col(|ui| {
                                        right(ui, fig(ov.and_then(|o| o.target_price).map(|v| format!("{v:.2}")).unwrap_or(blank.into())));
                                    });
                                    row.col(|ui| {
                                        match upside(ov, prices[i]) {
                                            Some(u) => right(ui, RichText::new(format!("{:+.1}%", u * 100.0)).font(theme::mono(12.5)).color(if u >= 0.0 { p.gain } else { p.loss })),
                                            None => right(ui, fig(blank.into())),
                                        }
                                    });
                                    row.col(|ui| match ov {
                                        Some(o) if o.analyst_count() > 0 => {
                                            let share = o.buy_share().unwrap_or(0.0);
                                            let tone = if share >= 0.6 { Tone::Gain } else if share >= 0.35 { Tone::Warn } else { Tone::Loss };
                                            theme::tone_pill(ui, &format!("{:.0}% buy", share * 100.0), tone).on_hover_text(format!(
                                                "{} analysts\nStrong buy {} · Buy {} · Hold {} · Sell {} · Strong sell {}",
                                                o.analyst_count(), o.strong_buy, o.buy, o.hold, o.sell, o.strong_sell
                                            ));
                                        }
                                        _ => {
                                            ui.label(RichText::new(blank).color(p.muted));
                                        }
                                    });
                                    row.col(|ui| {
                                        right(ui, fig(prices[i].map(|p| format!("{p:.2}")).unwrap_or("—".into())));
                                    });
                                    row.col(|ui| {
                                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                            ui.add(egui::DragValue::new(&mut line.weight_pct).range(0.0..=100.0).speed(0.1).suffix("%").max_decimals(2))
                                                .on_hover_text("Relative weight. Drag or click to type; the basket is rescaled to 100%.");
                                        });
                                    });
                                    row.col(|ui| {
                                        right(ui, fig(format!("{:.2}%", shares[i] * 100.0)));
                                    });
                                    row.col(|ui| {
                                        right(ui, fig(money(alloc[i].dollars)));
                                    });
                                    row.col(|ui| {
                                        if prices[i].is_some() {
                                            right(ui, RichText::new(fmt_qty(alloc[i].quantity)).font(theme::mono_semibold(12.5)));
                                        } else {
                                            right(ui, RichText::new("no price").color(p.warn));
                                        }
                                    });
                                    row.col(|ui| {
                                        if ui.small_button("Remove").on_hover_text("Take this stock out of the basket").clicked() {
                                            remove = Some(i);
                                        }
                                    });
                                });
                            }
                        });
                });
            });

            ui.add_space(8.0);
            ui.horizontal(|ui| {
                let resp = ui.add(egui::TextEdit::singleline(&mut review.add_input).hint_text("Ticker, e.g. MSFT").desired_width(130.0));
                let enter = resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                if (ui.button("Add to basket").clicked() || enter) && !review.add_input.trim().is_empty() {
                    add = Some(review.add_input.trim().to_uppercase());
                }
                ui.separator();
                if ui.button("Reset to index weights").on_hover_text("Weight each stock by its S&P 500 weight").clicked() {
                    reset_weights(&mut review.lines);
                }
                if let Some(msg) = &review.message {
                    ui.label(RichText::new(msg).color(p.warn));
                }
            });

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
                if let Some(l) = review.lines.last() {
                    self.fetch_overviews(vec![l.symbol.clone()]);
                }
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

/// Analyst target vs. current price (0.1 = 10% upside).
fn upside(ov: Option<&Overview>, price: Option<f64>) -> Option<f64> {
    let (t, p) = (ov?.target_price?, price?);
    (p > 0.0).then(|| t / p - 1.0)
}

fn sort_lines(
    lines: &mut [Line],
    market: &HashMap<String, MarketData>,
    overviews: &HashMap<String, Overview>,
    fits: &HashMap<String, Fit>,
    names: &HashMap<String, String>,
    (col, ascending): (usize, bool),
) {
    let name = |l: &Line| names.get(&l.symbol).cloned().unwrap_or_default();
    let ov = |l: &Line| overviews.get(&l.symbol);
    let price = |l: &Line| market.get(&l.symbol).and_then(|m| m.price);
    // Share and Amount are proportional to Weight, and Shares to Weight/Price,
    // so no need to compute the allocation to order by them.
    let num = |l: &Line| -> Option<f64> {
        match REVIEW_COLUMNS[col] {
            "Div Yld" => market.get(&l.symbol).and_then(|m| m.div_yield),
            "P/E" => market.get(&l.symbol).and_then(|m| m.pe_ratio),
            "Fwd P/E" => ov(l).and_then(|o| o.forward_pe),
            "Beta" => market.get(&l.symbol).and_then(|m| m.beta),
            "Alpha" => fits.get(&l.symbol).map(|f| f.alpha),
            "Target" => ov(l).and_then(|o| o.target_price),
            "Upside" => upside(ov(l), price(l)),
            "Analysts" => ov(l).and_then(Overview::buy_share),
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

fn right(ui: &mut egui::Ui, text: RichText) {
    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
        ui.label(text);
    });
}

/// Ticker, company name, optional value, and a labelled Remove button.
fn list_row(ui: &mut egui::Ui, symbol: &str, name: &str, value: Option<&str>, on_remove: impl FnOnce()) {
    let p = pal(ui);
    ui.horizontal(|ui| {
        ui.label(RichText::new(symbol).font(theme::mono_semibold(theme::BODY)));
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui.small_button("Remove").clicked() {
                on_remove();
            }
            if let Some(v) = value {
                ui.label(RichText::new(v).font(theme::mono(12.5)));
            }
            ui.add(egui::Label::new(RichText::new(name).color(p.muted)).truncate()).on_hover_text(name);
        });
    });
}
