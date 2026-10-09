//! Order placement UI: the confirm dialog on the Review screen and the
//! progress screen that follows each order to its final state.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use eframe::egui::{self, RichText};
use egui_extras::{Column, TableBuilder};
use schwab::{
    api::MarketData,
    execution::{ExecEvent, OrderState, Plan, PlannedOrder},
    orders::Side,
    universe::universe_symbol,
};

use crate::app::{money, IndexerApp};
use crate::theme::{self, pal, Tone};
use crate::home::fmt_qty;
use crate::modes::{Mode, Review};

pub struct Execution {
    pub preview: bool,
    pub sells: Vec<(PlannedOrder, OrderState)>,
    pub buys: Vec<(PlannedOrder, OrderState)>,
    pub notes: Vec<String>,
    pub running: bool,
    pub cancel: Arc<AtomicBool>,
}

impl IndexerApp {
    /// Rebalance sells: every position (all share classes) behind each
    /// discarded ticker, sold in full.
    pub(crate) fn planned_sells(&self, review: &Review, market: &HashMap<String, MarketData>) -> Vec<PlannedOrder> {
        let Some(acct) = &self.account.value else { return Vec::new() };
        let discards: Vec<&String> = review.sells.iter().map(|(s, _)| s).collect();
        acct.positions
            .iter()
            .filter(|p| p.long_quantity > 0.0 && p.asset_type == "EQUITY")
            .filter(|p| discards.contains(&&universe_symbol(&p.symbol)))
            .map(|p| {
                let symbol = universe_symbol(&p.symbol);
                let est_price = market
                    .get(&symbol)
                    .and_then(|m| m.price)
                    .unwrap_or(p.market_value / p.long_quantity);
                PlannedOrder { side: Side::Sell, symbol, schwab_symbol: p.symbol.clone(), quantity: p.long_quantity, est_price }
            })
            .collect()
    }

    /// The confirm dialog. Returns a plan to run once the user confirms.
    pub(crate) fn confirm_modal(
        &self,
        ui: &mut egui::Ui,
        review: &mut Review,
        buys: Vec<PlannedOrder>,
        skipped: &[String],
        market: &HashMap<String, MarketData>,
    ) -> Option<Plan> {
        let Some(mut preview) = review.confirm else { return None };
        let sells = self.planned_sells(review, market);
        let sell_total: f64 = sells.iter().map(PlannedOrder::est_value).sum();
        let buy_total: f64 = buys.iter().map(PlannedOrder::est_value).sum();
        let n_orders = sells.len() + buys.len();
        let mut confirmed = false;
        let mut cancel = false;

        let p = pal(ui);
        let resp = egui::Modal::new(egui::Id::new("confirm_orders")).show(ui.ctx(), |ui| {
            ui.set_width(600.0);
            ui.label(RichText::new("Confirm orders").font(theme::sans_semibold(theme::HEADING)));
            ui.label(RichText::new("Market orders, good for today. If the market is closed they execute at the next open.").color(p.muted));
            ui.add_space(10.0);

            theme::card(ui).inner_margin(egui::Margin::symmetric(12, 8)).show(ui, |ui| {
                egui::ScrollArea::vertical().max_height(340.0).show(ui, |ui| {
                    egui::Grid::new("confirm_grid").striped(true).num_columns(5).spacing([14.0, 6.0]).min_col_width(0.0).show(ui, |ui| {
                        for (side, list) in [(Side::Sell, &sells), (Side::Buy, &buys)] {
                            for o in list.iter() {
                                side_pill(ui, side);
                                ui.label(RichText::new(&o.symbol).font(theme::mono_semibold(theme::BODY)));
                                let name = self.display_name(&o.symbol);
                                ui.add(egui::Label::new(RichText::new(name).color(p.muted)).truncate());
                                ui.label(RichText::new(format!("{} sh", fmt_qty(o.quantity))).font(theme::mono(12.5)));
                                ui.label(RichText::new(format!("≈ {}", money(o.est_value()))).font(theme::mono(12.5)));
                                ui.end_row();
                            }
                        }
                    });
                });
            });
            ui.add_space(10.0);
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 30.0;
                if !sells.is_empty() {
                    theme::stat(ui, &format!("Sell · {} positions", sells.len()), &money(sell_total), Some(p.loss));
                }
                theme::stat(ui, &format!("Buy · {} stocks", buys.len()), &money(buy_total), None);
            });
            if !sells.is_empty() {
                ui.add_space(6.0);
                ui.label(RichText::new(
                    "Buys go out after the sells fill, scaled down if the sells raise less than planned. \
                     Keep the app open until the buys have been placed.",
                ).color(p.muted));
            }
            if !skipped.is_empty() {
                ui.label(RichText::new(format!("Skipped, no price or zero shares: {}", skipped.join(", "))).color(p.warn));
            }
            ui.add_space(10.0);
            ui.checkbox(&mut preview, "Preview only: build the orders but don't send them to Schwab");
            ui.add_space(12.0);
            ui.horizontal(|ui| {
                cancel = ui.button("Cancel").clicked();
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let (label, fill) = if preview {
                        (format!("Preview {n_orders} orders"), p.accent)
                    } else {
                        (format!("Send {n_orders} orders to Schwab"), p.loss)
                    };
                    let button = egui::Button::new(RichText::new(label).font(theme::sans_semibold(theme::BODY)).color(p.on_accent))
                        .fill(fill)
                        .min_size(egui::Vec2::new(0.0, 32.0));
                    confirmed = ui.add_enabled(n_orders > 0, button).clicked();
                });
            });
        });

        review.confirm = Some(preview);
        if cancel || resp.should_close() {
            review.confirm = None;
            return None;
        }
        if !confirmed {
            return None;
        }
        review.confirm = None;
        let account_number = self.account.value.as_ref()?.account_number.clone();
        Some(Plan {
            preview,
            planned_proceeds: if review.rebalance { sell_total } else { 0.0 },
            amount: buy_total,
            sells,
            buys,
            do_not_transact: self.dnt.iter().cloned().collect(),
            account_number,
        })
    }

    pub(crate) fn start_execution(&mut self, plan: Plan) {
        let cancel = Arc::new(AtomicBool::new(false));
        self.exec = Some(Execution {
            preview: plan.preview,
            sells: plan.sells.iter().map(|o| (o.clone(), OrderState::Pending)).collect(),
            buys: plan.buys.iter().map(|o| (o.clone(), OrderState::Pending)).collect(),
            notes: Vec::new(),
            running: true,
            cancel: cancel.clone(),
        });
        self.worker.run_orders(plan, cancel);
        self.mode = Mode::Execute;
    }

    pub(crate) fn handle_exec(&mut self, ev: ExecEvent) {
        let Some(exec) = &mut self.exec else { return };
        match ev {
            ExecEvent::BuysPlanned(buys) => exec.buys = buys.into_iter().map(|o| (o, OrderState::Pending)).collect(),
            ExecEvent::Sell(i, s) => {
                if let Some(row) = exec.sells.get_mut(i) {
                    row.1 = s;
                }
            }
            ExecEvent::Buy(i, s) => {
                if let Some(row) = exec.buys.get_mut(i) {
                    row.1 = s;
                }
            }
            ExecEvent::Note(n) => exec.notes.push(n),
            ExecEvent::Finished => {
                exec.running = false;
                if !exec.preview {
                    self.load_account();
                }
            }
        }
    }

    pub(crate) fn execute_ui(&mut self, ui: &mut egui::Ui) {
        let Some(exec) = &self.exec else {
            self.mode = Mode::Browse;
            return;
        };
        let p = pal(ui);
        let mut done = false;

        let frame = egui::Frame::new().fill(p.bg).inner_margin(egui::Margin::symmetric(22, 16));
        egui::CentralPanel::default().frame(frame).show(ui, |ui| {
            let rows: Vec<&(PlannedOrder, OrderState)> = exec.sells.iter().chain(&exec.buys).collect();
            let count = |f: &dyn Fn(&OrderState) -> bool| rows.iter().filter(|(_, s)| f(s)).count();
            let filled = count(&|s| matches!(s, OrderState::Placed { status: Some(st), .. } if st.status == "FILLED"));
            let failed = count(&|s| status_tone(s).1 == ToneKind::Bad);
            let working = count(&|s| matches!(s, OrderState::Placed { status, .. } if !status.as_ref().is_some_and(|st| st.is_terminal())));

            ui.horizontal(|ui| {
                let title = if exec.preview { "Order preview" } else { "Placing orders" };
                ui.label(RichText::new(title).font(theme::sans_semibold(theme::HEADING)));
                if exec.preview {
                    theme::tone_pill(ui, "Nothing was sent", Tone::Neutral);
                }
                if exec.running {
                    ui.spinner();
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if exec.running {
                        let label = if exec.buys.iter().any(|(_, s)| !matches!(s, OrderState::Pending)) {
                            "Stop tracking"
                        } else {
                            "Stop waiting (don't place buys)"
                        };
                        if ui.button(label).clicked() {
                            exec.cancel.store(true, Ordering::Relaxed);
                        }
                    } else {
                        done = ui.add(theme::primary(ui, "Done").min_size(egui::Vec2::new(100.0, 32.0))).clicked();
                    }
                });
            });
            ui.add_space(10.0);

            if !exec.preview {
                theme::card(ui).inner_margin(egui::Margin::symmetric(18, 12)).show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = 34.0;
                        theme::stat(ui, "Orders", &rows.len().to_string(), None);
                        theme::stat(ui, "Filled", &filled.to_string(), Some(p.gain));
                        theme::stat(ui, "Working", &working.to_string(), Some(p.warn));
                        theme::stat(ui, "Failed", &failed.to_string(), Some(if failed > 0 { p.loss } else { p.muted }));
                    });
                });
                ui.add_space(8.0);
            }
            for n in &exec.notes {
                ui.label(RichText::new(n).color(p.ink));
            }
            ui.add_space(4.0);

            theme::card(ui).inner_margin(egui::Margin::symmetric(14, 8)).show(ui, |ui| {
                egui::ScrollArea::both().max_height(ui.available_height() - 40.0).auto_shrink([false, true]).show(ui, |ui| {
                    TableBuilder::new(ui)
                        .id_salt("exec_table")
                        .striped(true)
                        .vscroll(false)
                        .cell_layout(egui::Layout::left_to_right(egui::Align::Center))
                        .column(Column::exact(58.0))
                        .column(Column::exact(70.0))
                        .column(Column::initial(210.0).clip(true))
                        .columns(Column::initial(96.0), 5)
                        .column(Column::remainder().at_least(160.0).clip(true))
                        .header(26.0, |mut h| {
                            for label in ["Side", "Ticker", "Company", "Shares", "Est. value", "Status", "Filled", "Avg price", "Detail"] {
                                h.col(|ui| {
                                    ui.label(theme::eyebrow(ui, label));
                                });
                            }
                        })
                        .body(|mut body| {
                            for (o, state) in &rows {
                                body.row(28.0, |mut row| {
                                    row.col(|ui| {
                                        side_pill(ui, o.side);
                                    });
                                    row.col(|ui| {
                                        ui.label(RichText::new(&o.symbol).font(theme::mono_semibold(13.0)));
                                    });
                                    row.col(|ui| {
                                        let name = self.display_name(&o.symbol);
                                        ui.label(&name).on_hover_text(&name);
                                    });
                                    row.col(|ui| {
                                        ui.label(RichText::new(fmt_qty(o.quantity)).font(theme::mono(12.5)));
                                    });
                                    row.col(|ui| {
                                        ui.label(RichText::new(money(o.est_value())).font(theme::mono(12.5)));
                                    });
                                    status_cells(&mut row, state);
                                });
                            }
                        });

                    if exec.preview {
                        ui.add_space(10.0);
                        egui::CollapsingHeader::new(RichText::new("Show the request bodies that would be sent").color(p.accent)).show(ui, |ui| {
                            for (o, state) in &rows {
                                if let OrderState::Preview(body) = state {
                                    ui.label(RichText::new(format!("{} {}", o.side.instruction(), o.symbol)).font(theme::mono_semibold(12.5)));
                                    ui.code(serde_json::to_string_pretty(body).unwrap_or_default());
                                }
                            }
                        });
                    }
                });
            });
            if !exec.preview {
                ui.add_space(6.0);
                ui.label(RichText::new("Every order and its result is saved to order-log.jsonl.").color(p.muted).font(theme::sans(theme::SMALL)));
            }
        });

        if done {
            if !exec.preview {
                self.basket.clear();
                self.discards.clear();
                self.review = None;
            }
            self.exec = None;
            self.mode = if self.review.is_some() { Mode::Review } else { Mode::Browse };
        }
    }
}

#[derive(PartialEq)]
enum ToneKind {
    Good,
    Bad,
    Busy,
    Idle,
}

fn status_tone(state: &OrderState) -> (String, ToneKind) {
    match state {
        OrderState::Pending => ("Waiting".into(), ToneKind::Idle),
        OrderState::Preview(_) => ("Preview".into(), ToneKind::Idle),
        OrderState::Placed { status: None, .. } => ("Sent".into(), ToneKind::Busy),
        OrderState::Placed { status: Some(st), .. } => {
            let kind = match st.status.as_str() {
                "FILLED" => ToneKind::Good,
                "REJECTED" | "CANCELED" | "EXPIRED" => ToneKind::Bad,
                _ => ToneKind::Busy,
            };
            (title_case(&st.status), kind)
        }
        OrderState::Failed(_) => ("Failed".into(), ToneKind::Bad),
    }
}

/// "PENDING_ACTIVATION" → "Pending activation".
fn title_case(status: &str) -> String {
    let lower = status.replace('_', " ").to_lowercase();
    let mut c = lower.chars();
    c.next().map(|f| f.to_uppercase().collect::<String>() + c.as_str()).unwrap_or_default()
}

fn side_pill(ui: &mut egui::Ui, side: Side) {
    match side {
        Side::Buy => theme::tone_pill(ui, "BUY", Tone::Gain),
        Side::Sell => theme::tone_pill(ui, "SELL", Tone::Loss),
    };
}

fn status_cells(row: &mut egui_extras::TableRow<'_, '_>, state: &OrderState) {
    let (label, kind) = status_tone(state);
    let (filled, avg, detail) = match state {
        OrderState::Placed { status: Some(st), .. } => (
            fmt_qty(st.filled_quantity),
            st.avg_price().map(|p| format!("{p:.4}")).unwrap_or_default(),
            st.description.clone().unwrap_or_default(),
        ),
        OrderState::Failed(e) => (String::new(), String::new(), e.clone()),
        OrderState::Preview(_) => (String::new(), String::new(), "Not sent".into()),
        _ => (String::new(), String::new(), String::new()),
    };
    row.col(|ui| {
        let tone = match kind {
            ToneKind::Good => Tone::Gain,
            ToneKind::Bad => Tone::Loss,
            ToneKind::Busy => Tone::Warn,
            ToneKind::Idle => Tone::Neutral,
        };
        theme::tone_pill(ui, &label, tone);
    });
    row.col(|ui| {
        ui.label(RichText::new(filled).font(theme::mono(12.5)));
    });
    row.col(|ui| {
        ui.label(RichText::new(avg).font(theme::mono(12.5)));
    });
    row.col(|ui| {
        let color = if kind == ToneKind::Bad { pal(ui).loss } else { pal(ui).muted };
        ui.label(RichText::new(&detail).color(color)).on_hover_text(&detail);
    });
}
