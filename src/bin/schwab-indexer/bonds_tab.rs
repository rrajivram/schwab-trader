//! Bonds tab: what each bond pays back and when — maturities and remaining
//! coupons, soonest first — plus a sortable holdings table.

use std::cmp::Ordering;

use chrono::{Local, NaiveDate};
use eframe::egui::{self, Margin, RichText};
use egui_extras::{Column, TableBuilder};
use schwab::bonds::{self, Bond, PaymentKind};

use crate::app::{money, IndexerApp};
use crate::theme::{self, pal, Tone};

/// Payments this close are called out.
pub const SOON_DAYS: i64 = 30;

const HOLDING_COLUMNS: [&str; 11] = [
    "Bond", "CUSIP", "Coupon", "Matures", "Days left", "Face", "Cost", "Market value", "Coupons left", "Total to receive",
    "Gain to maturity",
];

pub fn today() -> NaiveDate {
    Local::now().date_naive()
}

/// Payments arriving within `SOON_DAYS`, for the tab badge.
pub fn soon_count(app: &IndexerApp) -> usize {
    let Some(acct) = &app.account.value else { return 0 };
    let t = today();
    bonds::upcoming(&bonds::bonds_in(acct), t)
        .iter()
        .filter(|p| (p.paid - t).num_days() <= SOON_DAYS)
        .count()
}

fn days_label(days: i64) -> String {
    match days {
        0 => "today".into(),
        1 => "tomorrow".into(),
        d => format!("in {d} days"),
    }
}

impl IndexerApp {
    pub(crate) fn bonds_ui(&mut self, ui: &mut egui::Ui) {
        let p = pal(ui);
        let frame = egui::Frame::new().fill(p.bg).inner_margin(Margin::symmetric(18, 14));
        egui::CentralPanel::default().frame(frame).show(ui, |ui| {
            let Some(acct) = &self.account.value else {
                ui.horizontal(|ui| {
                    if self.account.loading {
                        ui.spinner();
                        ui.label("Loading your account…");
                    } else {
                        ui.label("No account loaded.");
                    }
                });
                return;
            };
            let t = today();
            let held = bonds::bonds_in(acct);
            if held.is_empty() {
                ui.label(RichText::new("No bonds in this account.").color(p.muted));
                return;
            }
            let pays = bonds::upcoming(&held, t);
            let within = |d: i64| pays.iter().filter(|x| (x.paid - t).num_days() <= d).map(|x| x.amount).sum::<f64>();
            let face: f64 = held.iter().map(|b| b.face).sum();
            let mv: f64 = held.iter().map(|b| b.market_value).sum();
            let coupons: f64 = held.iter().map(|b| b.remaining_coupons(t)).sum();

            egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
                ui.spacing_mut().item_spacing.y = 12.0;

                // Summary.
                theme::card(ui).inner_margin(Margin::symmetric(18, 12)).show(ui, |ui| {
                    ui.horizontal_wrapped(|ui| {
                        ui.spacing_mut().item_spacing.x = 34.0;
                        theme::stat(ui, &format!("{} bonds · face value", held.len()), &money(face), None);
                        theme::stat(ui, "Market value", &money(mv), None);
                        theme::stat(ui, "Coming in 30 days", &money(within(30)), Some(if within(30) > 0.0 { p.accent } else { p.muted }));
                        theme::stat(ui, "Coming in 90 days", &money(within(90)), None);
                        theme::stat(ui, "Coupons still to come", &money(coupons), Some(p.gain));
                    });
                });

                // Coming up.
                theme::card(ui).inner_margin(Margin::symmetric(14, 10)).show(ui, |ui| {
                    ui.label(RichText::new("Coming up").font(theme::sans_semibold(theme::TITLE)));
                    ui.label(
                        RichText::new("Payments that land on a weekend or federal holiday arrive the next business day.")
                            .color(p.muted)
                            .font(theme::sans(theme::SMALL)),
                    );
                    ui.add_space(4.0);
                    TableBuilder::new(ui)
                        .id_salt("bond_payments")
                        .striped(true)
                        .vscroll(false)
                        .cell_layout(egui::Layout::left_to_right(egui::Align::Center))
                        .column(Column::exact(150.0))
                        .column(Column::exact(110.0))
                        .column(Column::exact(90.0))
                        .column(Column::initial(240.0).clip(true))
                        .column(Column::exact(96.0))
                        .column(Column::initial(110.0))
                        .header(24.0, |mut h| {
                            for l in ["Arrives", "", "Type", "Bond", "CUSIP", "Amount"] {
                                h.col(|ui| {
                                    ui.label(theme::eyebrow(ui, l));
                                });
                            }
                        })
                        .body(|mut body| {
                            for x in &pays {
                                let days = (x.paid - t).num_days();
                                let soon = days <= SOON_DAYS;
                                body.row(28.0, |mut row| {
                                    row.col(|ui| {
                                        if soon {
                                            theme::row_tint(ui, p.warn_soft);
                                        }
                                        let r = ui.label(RichText::new(x.paid.format("%a %b %-d, %Y").to_string()).font(theme::mono(12.5)));
                                        if x.paid != x.scheduled {
                                            r.on_hover_text(format!("Scheduled {}, a weekend or holiday", x.scheduled.format("%a %b %-d")));
                                        }
                                    });
                                    row.col(|ui| {
                                        if soon {
                                            theme::row_tint(ui, p.warn_soft);
                                            theme::tone_pill(ui, &days_label(days), Tone::Warn);
                                        } else {
                                            ui.label(RichText::new(days_label(days)).color(p.muted));
                                        }
                                    });
                                    row.col(|ui| {
                                        if soon {
                                            theme::row_tint(ui, p.warn_soft);
                                        }
                                        match x.kind {
                                            PaymentKind::Maturity => theme::tone_pill(ui, "Maturity", Tone::Accent),
                                            PaymentKind::Coupon => theme::tone_pill(ui, "Coupon", Tone::Gain),
                                        };
                                    });
                                    row.col(|ui| {
                                        if soon {
                                            theme::row_tint(ui, p.warn_soft);
                                        }
                                        ui.label(&x.description);
                                    });
                                    row.col(|ui| {
                                        if soon {
                                            theme::row_tint(ui, p.warn_soft);
                                        }
                                        ui.label(RichText::new(&x.cusip).font(theme::mono(12.5)).color(p.muted));
                                    });
                                    row.col(|ui| {
                                        if soon {
                                            theme::row_tint(ui, p.warn_soft);
                                        }
                                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                            ui.label(RichText::new(money(x.amount)).font(theme::mono_semibold(13.0)));
                                        });
                                    });
                                });
                            }
                        });
                });

                // Holdings.
                let mut rows: Vec<&Bond> = held.iter().collect();
                if let Some((col, asc)) = self.bond_sort {
                    rows.sort_by(|a, b| {
                        let o = compare(a, b, col, t);
                        if asc { o } else { o.reverse() }
                    });
                } else {
                    rows.sort_by_key(|b| b.maturity);
                }
                let mut clicked = None;
                theme::card(ui).inner_margin(Margin::symmetric(14, 10)).show(ui, |ui| {
                    ui.label(RichText::new("Holdings").font(theme::sans_semibold(theme::TITLE)));
                    ui.add_space(4.0);
                    egui::ScrollArea::horizontal().id_salt("bond_holdings_scroll").show(ui, |ui| {
                        TableBuilder::new(ui)
                            .id_salt("bond_holdings")
                            .striped(true)
                            .vscroll(false)
                            .cell_layout(egui::Layout::left_to_right(egui::Align::Center))
                            .column(Column::initial(220.0).clip(true))
                            .column(Column::exact(96.0))
                            .columns(Column::initial(92.0), 3)
                            .columns(Column::initial(104.0), 6)
                            .header(26.0, |mut h| {
                                for (i, label) in HOLDING_COLUMNS.iter().enumerate() {
                                    h.col(|ui| {
                                        let active = self.bond_sort.and_then(|(c, a)| (c == i).then_some(a));
                                        if theme::sort_header(ui, label, active, "") {
                                            clicked = Some(i);
                                        }
                                    });
                                }
                            })
                            .body(|mut body| {
                                for b in &rows {
                                    let left = (b.maturity - t).num_days();
                                    let coupons = b.remaining_coupons(t);
                                    let fig = |s: String| RichText::new(s).font(theme::mono(12.5));
                                    body.row(28.0, |mut row| {
                                        row.col(|ui| {
                                            ui.label(&b.description).on_hover_text(&b.description);
                                        });
                                        row.col(|ui| {
                                            ui.label(RichText::new(&b.cusip).font(theme::mono(12.5)).color(p.muted));
                                        });
                                        row.col(|ui| {
                                            if b.is_bill() {
                                                ui.label(RichText::new("Bill").color(p.muted));
                                            } else {
                                                ui.label(fig(format!("{:.3}%", b.coupon_rate)));
                                            }
                                        });
                                        row.col(|ui| {
                                            ui.label(fig(b.maturity.format("%b %-d, %Y").to_string()));
                                        });
                                        row.col(|ui| {
                                            if left <= SOON_DAYS {
                                                theme::tone_pill(ui, &format!("{left} days"), Tone::Warn);
                                            } else {
                                                ui.label(fig(format!("{left}")));
                                            }
                                        });
                                        for v in [b.face, b.cost, b.market_value] {
                                            row.col(|ui| right(ui, fig(money(v))));
                                        }
                                        row.col(|ui| right(ui, fig(if b.is_bill() { "—".into() } else { money(coupons) })));
                                        row.col(|ui| right(ui, RichText::new(money(b.face + coupons)).font(theme::mono_semibold(12.5))));
                                        row.col(|ui| {
                                            let g = b.face + coupons - b.cost;
                                            right(ui, fig(money(g)).color(if g >= 0.0 { p.gain } else { p.loss }));
                                        });
                                    });
                                }
                            });
                    });
                    ui.label(
                        RichText::new(
                            "Bills pay no coupon; their return is face value minus what you paid. \
                             Gain to maturity = face + coupons still to come − cost.",
                        )
                        .color(p.muted)
                        .font(theme::sans(theme::SMALL)),
                    );
                });
                if let Some(col) = clicked {
                    self.bond_sort = Some(match self.bond_sort {
                        Some((c, asc)) if c == col => (col, !asc),
                        // Text and dates ascend first; amounts largest first.
                        _ => (col, matches!(col, 0 | 1 | 3 | 4)),
                    });
                }
            });
        });
    }
}

fn right(ui: &mut egui::Ui, text: RichText) {
    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
        ui.label(text);
    });
}

fn compare(a: &Bond, b: &Bond, col: usize, t: NaiveDate) -> Ordering {
    let num = |x: &Bond| -> f64 {
        let c = x.remaining_coupons(t);
        match col {
            2 => x.coupon_rate,
            5 => x.face,
            6 => x.cost,
            7 => x.market_value,
            8 => c,
            9 => x.face + c,
            10 => x.face + c - x.cost,
            _ => 0.0,
        }
    };
    match col {
        0 => a.description.cmp(&b.description),
        1 => a.cusip.cmp(&b.cusip),
        3 | 4 => a.maturity.cmp(&b.maturity),
        _ => num(a).total_cmp(&num(b)),
    }
    .then_with(|| a.maturity.cmp(&b.maturity))
}
