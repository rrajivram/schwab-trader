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

        let resp = egui::Modal::new(egui::Id::new("confirm_orders")).show(ui.ctx(), |ui| {
            ui.set_width(560.0);
            ui.heading("Confirm orders");
            ui.label("Market orders, good for the day. If the market is closed they execute at the next open.");
            ui.add_space(6.0);

            egui::ScrollArea::vertical().max_height(360.0).show(ui, |ui| {
                egui::Grid::new("confirm_grid").striped(true).num_columns(5).spacing([14.0, 4.0]).show(ui, |ui| {
                    for (label, list) in [("SELL", &sells), ("BUY", &buys)] {
                        for o in list.iter() {
                            ui.strong(label);
                            ui.monospace(&o.symbol);
                            let name = market.get(&o.symbol).and_then(|m| m.description.as_deref()).unwrap_or("");
                            ui.add(egui::Label::new(RichText::new(name).weak()).truncate());
                            ui.label(format!("{} sh", fmt_qty(o.quantity)));
                            ui.label(format!("≈ {}", money(o.est_value())));
                            ui.end_row();
                        }
                    }
                });
            });
            ui.separator();
            if !sells.is_empty() {
                ui.label(format!("Sell {} positions ≈ {}", sells.len(), money(sell_total)));
                ui.label("Buys are placed after the sells fill. If the sells raise less than planned, buys are scaled down.");
                ui.label("Keep the app open until the buys have been placed.");
            }
            ui.strong(format!("Buy {} stocks ≈ {}", buys.len(), money(buy_total)));
            if !skipped.is_empty() {
                ui.colored_label(ui.visuals().warn_fg_color, format!("Skipped (no price or zero shares): {}", skipped.join(", ")));
            }
            ui.add_space(8.0);
            ui.checkbox(&mut preview, "Preview only — build the orders but don't send them to Schwab");
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                cancel = ui.button("Cancel").clicked();
                let label = if preview {
                    format!("Preview {n_orders} orders")
                } else {
                    format!("Send {n_orders} orders to Schwab")
                };
                let button = egui::Button::new(RichText::new(label).strong());
                let button = if preview { button } else { button.fill(ui.visuals().error_fg_color.gamma_multiply(0.35)) };
                confirmed = ui.add_enabled(n_orders > 0, button).clicked();
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
        Some(Plan {
            preview,
            planned_proceeds: if review.rebalance { sell_total } else { 0.0 },
            amount: buy_total,
            sells,
            buys,
            do_not_transact: self.dnt.iter().cloned().collect(),
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
        let market = self.market.value.as_ref();
        let mut done = false;

        egui::CentralPanel::default().show(ui, |ui| {
            ui.horizontal(|ui| {
                if exec.preview {
                    ui.heading("Order preview — nothing was sent");
                } else {
                    ui.heading("Placing orders");
                }
                if exec.running {
                    ui.spinner();
                }
            });
            for n in &exec.notes {
                ui.label(n);
            }
            ui.add_space(6.0);

            let rows: Vec<&(PlannedOrder, OrderState)> = exec.sells.iter().chain(&exec.buys).collect();
            egui::ScrollArea::vertical().max_height(ui.available_height() - 60.0).show(ui, |ui| {
                TableBuilder::new(ui)
                    .id_salt("exec_table")
                    .striped(true)
                    .vscroll(false)
                    .cell_layout(egui::Layout::left_to_right(egui::Align::Center))
                    .column(Column::exact(44.0))
                    .column(Column::exact(64.0))
                    .column(Column::initial(200.0).clip(true))
                    .columns(Column::initial(90.0), 5)
                    .column(Column::remainder().clip(true))
                    .header(22.0, |mut h| {
                        for label in ["Side", "Symbol", "Company", "Shares", "Est. value", "Status", "Filled", "Avg price", "Detail"] {
                            h.col(|ui| {
                                ui.strong(label);
                            });
                        }
                    })
                    .body(|mut body| {
                        for (o, state) in &rows {
                            body.row(22.0, |mut row| {
                                row.col(|ui| {
                                    ui.strong(o.side.instruction());
                                });
                                row.col(|ui| {
                                    ui.monospace(&o.symbol);
                                });
                                row.col(|ui| {
                                    let name = market.and_then(|m| m.get(&o.symbol)?.description.as_deref()).unwrap_or("");
                                    ui.label(name).on_hover_text(name);
                                });
                                row.col(|ui| {
                                    ui.label(fmt_qty(o.quantity));
                                });
                                row.col(|ui| {
                                    ui.label(money(o.est_value()));
                                });
                                status_cells(&mut row, state);
                            });
                        }
                    });

                if exec.preview {
                    ui.add_space(8.0);
                    egui::CollapsingHeader::new("Request bodies that would be sent").show(ui, |ui| {
                        for (o, state) in &rows {
                            if let OrderState::Preview(body) = state {
                                ui.monospace(format!("{} {}", o.side.instruction(), o.symbol));
                                ui.code(serde_json::to_string_pretty(body).unwrap_or_default());
                            }
                        }
                    });
                }
            });

            ui.separator();
            ui.horizontal(|ui| {
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
                    done = ui.button("Done").clicked();
                }
                if !exec.preview {
                    ui.weak("Every order and its result is logged to order-log.jsonl.");
                }
            });
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

fn status_cells(row: &mut egui_extras::TableRow<'_, '_>, state: &OrderState) {
    let (status, filled, avg, detail): (String, String, String, String) = match state {
        OrderState::Pending => ("Waiting".into(), String::new(), String::new(), String::new()),
        OrderState::Preview(_) => ("Preview".into(), String::new(), String::new(), "not sent".into()),
        OrderState::Placed { status: None, .. } => ("Sent".into(), String::new(), String::new(), String::new()),
        OrderState::Placed { status: Some(st), .. } => (
            st.status.clone(),
            fmt_qty(st.filled_quantity),
            st.avg_price().map(|p| format!("{p:.4}")).unwrap_or_default(),
            st.description.clone().unwrap_or_default(),
        ),
        OrderState::Failed(e) => ("FAILED".into(), String::new(), String::new(), e.clone()),
    };
    let failed = matches!(state, OrderState::Failed(_))
        || matches!(state, OrderState::Placed { status: Some(st), .. } if matches!(st.status.as_str(), "REJECTED" | "CANCELED" | "EXPIRED"));
    row.col(|ui| {
        if failed {
            ui.colored_label(ui.visuals().error_fg_color, &status);
        } else {
            ui.label(&status);
        }
    });
    row.col(|ui| {
        ui.label(&filled);
    });
    row.col(|ui| {
        ui.label(&avg);
    });
    row.col(|ui| {
        ui.label(&detail).on_hover_text(&detail);
    });
}
