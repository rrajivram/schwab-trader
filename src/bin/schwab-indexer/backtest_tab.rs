//! Backtest tab: replays the picking rule (momentum + low beta, price-only)
//! over the cached weekly history and compares it with SPY and with an
//! equal-weight basket of the same stocks.

use chrono::DateTime;
use eframe::egui::{self, Color32, CornerRadius, Margin, Pos2, Rect, RichText, Sense, Shape, Stroke, Vec2};
use egui_extras::{Column, TableBuilder};
use schwab::backtest::{Outcome, Params, Stats};

use crate::app::{money, IndexerApp};
use crate::theme::{self, pal, Tone};

/// Rebalance choices: (label, weeks).
const SCHEDULES: [(&str, usize); 3] = [("Monthly", 4), ("Quarterly", 13), ("Yearly", 52)];
/// The chart shows the growth of this much money.
const START: f64 = 10_000.0;
const SERIES: [&str; 3] = ["Strategy", "Equal-weight survivors", "SPY"];

pub struct BacktestView {
    pub size: usize,
    pub every_weeks: usize,
    /// Momentum's share of the mix, 0–100; low beta gets the rest.
    pub momentum_pct: f64,
    pub running: bool,
    pub outcome: Option<Option<Outcome>>,
    /// Params of the shown outcome, to flag when the controls have moved on.
    pub ran: Option<Params>,
    pub error: Option<String>,
}

impl Default for BacktestView {
    fn default() -> Self {
        Self { size: 20, every_weeks: 13, momentum_pct: 50.0, running: false, outcome: None, ran: None, error: None }
    }
}

fn date(ms: i64) -> String {
    DateTime::from_timestamp_millis(ms).map(|d| d.format("%b %-d, %Y").to_string()).unwrap_or_default()
}

fn pct(v: f64) -> String {
    format!("{:+.1}%", v * 100.0)
}

impl IndexerApp {
    fn backtest_params(&self) -> Option<Params> {
        let b = &self.backtest;
        Some(Params {
            size: b.size,
            every_weeks: b.every_weeks,
            momentum_weight: b.momentum_pct,
            low_beta_weight: 100.0 - b.momentum_pct,
            rf_annual: *self.risk_free.value.as_ref()?,
        })
    }

    fn start_backtest(&mut self) {
        let (Some(params), Some(u)) = (self.backtest_params(), self.universe.value.clone()) else { return };
        let Some(bench) = self.history.get(schwab::history::BENCHMARK) else { return };
        let series = self.history.iter().map(|(k, v)| (k.clone(), v.closes.clone())).collect();
        self.backtest.running = true;
        self.backtest.error = None;
        self.worker.run_backtest(u, series, bench.closes.clone(), params);
    }

    pub(crate) fn finish_backtest(&mut self, params: Params, outcome: Option<Outcome>) {
        self.backtest.running = false;
        self.backtest.error = outcome.is_none().then(|| "Not enough price history to backtest yet.".to_string());
        self.backtest.ran = Some(params);
        self.backtest.outcome = Some(outcome);
    }

    pub(crate) fn backtest_ui(&mut self, ui: &mut egui::Ui) {
        let p = pal(ui);
        // Ready once prices are in and the risk-free rate (for Sharpe) is known.
        let ready = !self.history_running && self.history.contains_key(schwab::history::BENCHMARK) && self.backtest_params().is_some();
        if ready && self.backtest.outcome.is_none() && !self.backtest.running {
            self.start_backtest();
        }
        let mut run = false;
        let frame = egui::Frame::new().fill(p.bg).inner_margin(Margin::symmetric(18, 14));
        egui::CentralPanel::default().frame(frame).show(ui, |ui| {
            egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
                ui.spacing_mut().item_spacing.y = 12.0;
                run = self.backtest_controls(ui, ready);
                if let Some(e) = &self.backtest.error {
                    ui.colored_label(p.warn, e);
                }
                if let Some(Some(o)) = &self.backtest.outcome {
                    headline(ui, o);
                    chart_card(ui, o);
                    metrics_card(ui, o);
                    caveats_card(ui, o);
                    rebalances_card(ui, o, &self.names_all(o));
                } else if !ready {
                    theme::card(ui).show(ui, |ui| {
                        ui.horizontal(|ui| {
                            ui.spinner();
                            let (done, total) = self.history_progress;
                            let msg = if self.history_running {
                                format!("Loading 10 years of weekly prices ({done}/{total}). This runs about once a week and takes a few minutes.")
                            } else {
                                "Waiting for prices and the risk-free rate…".to_string()
                            };
                            ui.label(RichText::new(msg).color(p.muted));
                        });
                    });
                }
            });
        });
        if run {
            self.start_backtest();
        }
    }

    fn names_all(&self, o: &Outcome) -> std::collections::HashMap<String, String> {
        self.names(o.rebalances.iter().flat_map(|r| r.picks.iter()))
    }

    /// Returns true when Run was clicked.
    fn backtest_controls(&mut self, ui: &mut egui::Ui, ready: bool) -> bool {
        let p = pal(ui);
        let mut run = false;
        theme::card(ui).inner_margin(Margin::symmetric(18, 12)).show(ui, |ui| {
            ui.horizontal_wrapped(|ui| {
                ui.spacing_mut().item_spacing.x = 10.0;
                let b = &mut self.backtest;
                ui.label(theme::eyebrow(ui, "Basket"));
                ui.add(egui::DragValue::new(&mut b.size).range(1..=100).suffix(" stocks"));
                ui.add_space(14.0);

                ui.label(theme::eyebrow(ui, "Rebalance"));
                for (label, weeks) in SCHEDULES {
                    let on = b.every_weeks == weeks;
                    if theme::toggle_button(ui, on, label, label, Tone::Accent, true).clicked() {
                        b.every_weeks = weeks;
                    }
                }
                ui.add_space(14.0);

                ui.label(theme::eyebrow(ui, "Mix"));
                ui.label("Low beta");
                ui.spacing_mut().slider_width = 150.0;
                ui.add(egui::Slider::new(&mut b.momentum_pct, 0.0..=100.0).show_value(false)).on_hover_text(format!(
                    "Momentum {:.0}% · Low beta {:.0}%. Value and quality can't be backtested: Schwab only has today's fundamentals.",
                    b.momentum_pct,
                    100.0 - b.momentum_pct
                ));
                ui.label("Momentum");
                ui.label(RichText::new(format!("{:.0} / {:.0}", 100.0 - b.momentum_pct, b.momentum_pct)).font(theme::mono(12.5)).color(p.muted));

                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let label = if self.backtest.running { "Running…" } else { "Run backtest" };
                    run = ui
                        .add_enabled(ready && !self.backtest.running, theme::primary(ui, label).min_size(Vec2::new(130.0, 30.0)))
                        .on_disabled_hover_text("Waiting for price history")
                        .clicked();
                    if self.backtest.running {
                        ui.spinner();
                    }
                });
            });
            let stale = self.backtest.ran.is_some() && self.backtest.ran != self.backtest_params();
            if stale && !self.backtest.running {
                ui.label(RichText::new("Settings changed. Run again to update the results.").color(p.warn).font(theme::sans(theme::SMALL)));
            }
        });
        run
    }
}

fn headline(ui: &mut egui::Ui, o: &Outcome) {
    let p = pal(ui);
    let (s, e, b) = (&o.strategy.stats, &o.equal_weight.stats, &o.benchmark.stats);
    theme::card(ui).inner_margin(Margin::symmetric(18, 12)).show(ui, |ui| {
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing.x = 34.0;
            theme::stat(ui, "Strategy · per year", &pct(s.cagr), None);
            theme::stat(ui, "SPY · per year", &pct(b.cagr), None);
            let vs = |d: f64| Some(if d >= 0.0 { p.gain } else { p.loss });
            theme::stat(ui, "vs SPY", &format!("{} / yr", pct(s.cagr - b.cagr)), vs(s.cagr - b.cagr))
                .on_hover_text("Includes the survivorship head start, so it overstates what the picking added.");
            theme::stat(ui, "vs equal-weight survivors", &format!("{} / yr", pct(s.cagr - e.cagr)), vs(s.cagr - e.cagr)).on_hover_text(
                "Same stocks available, same schedule, no picking. This gap is the fairest measure of what the factor picking added.",
            );
            if let Some(f) = s.fit {
                let verdict = if f.significant() { "stands out from noise" } else { "can't be told from noise" };
                theme::stat(ui, "Alpha vs SPY", &format!("{} · t {:.1}", pct(f.alpha), f.alpha_t), Some(if f.significant() { p.ink } else { p.muted }))
                    .on_hover_text(format!("Return beyond what the strategy's beta of {:.2} explains; {verdict}.", f.beta));
            }
        });
        ui.label(
            RichText::new(format!(
                "{} to {} · {} rebalances · {} stocks with history · price-only, equal-weighted",
                date(o.dates[0]),
                date(*o.dates.last().unwrap()),
                o.rebalances.len(),
                o.universe_size
            ))
            .color(p.muted)
            .font(theme::sans(theme::SMALL)),
        );
    });
}

/// Round step for about `target` gridlines across `span`.
fn nice_step(span: f64, target: f64) -> f64 {
    let raw = span / target;
    let mag = 10f64.powf(raw.log10().floor());
    [1.0, 2.0, 2.5, 5.0, 10.0].iter().map(|m| m * mag).find(|s| *s >= raw).unwrap_or(10.0 * mag)
}

fn chart_card(ui: &mut egui::Ui, o: &Outcome) {
    let p = pal(ui);
    let curves = [&o.strategy.values, &o.equal_weight.values, &o.benchmark.values];
    theme::card(ui).inner_margin(Margin::symmetric(16, 12)).show(ui, |ui| {
        ui.horizontal(|ui| {
            ui.label(RichText::new(format!("Growth of {}", money(START))).font(theme::sans_semibold(theme::TITLE)));
            ui.add_space(16.0);
            // Legend: a short line key per series; text stays in ink.
            for (i, name) in SERIES.iter().enumerate() {
                let (r, _) = ui.allocate_exact_size(Vec2::new(16.0, 12.0), Sense::hover());
                ui.painter().line_segment([r.left_center(), r.right_center()], Stroke::new(2.0, p.series[i]));
                ui.label(RichText::new(*name).color(p.ink));
                ui.add_space(8.0);
            }
        });
        ui.add_space(4.0);

        let width = ui.available_width();
        let (rect, resp) = ui.allocate_exact_size(Vec2::new(width, 340.0), Sense::hover());
        let plot = Rect::from_min_max(rect.min + Vec2::new(58.0, 8.0), rect.max - Vec2::new(150.0, 24.0));
        let painter = ui.painter_at(rect);
        let n = o.dates.len();
        let (lo, hi) = curves.iter().flat_map(|c| c.iter()).fold((f64::MAX, f64::MIN), |(lo, hi), v| (lo.min(*v), hi.max(*v)));
        let (lo, hi) = (lo * START, hi * START);
        let step = nice_step(hi - lo, 5.0);
        let (y0, y1) = ((lo / step).floor() * step, (hi / step).ceil() * step);
        let x = |i: usize| plot.left() + plot.width() * i as f32 / (n - 1).max(1) as f32;
        let y = |v: f64| plot.bottom() - plot.height() * ((v - y0) / (y1 - y0)) as f32;
        let small = theme::sans(theme::SMALL);

        // Recessive hairline grid and axis labels.
        let mut g = y0;
        while g <= y1 + step * 0.01 {
            painter.line_segment([Pos2::new(plot.left(), y(g)), Pos2::new(plot.right(), y(g))], Stroke::new(1.0, p.border));
            painter.text(Pos2::new(plot.left() - 8.0, y(g)), egui::Align2::RIGHT_CENTER, money(g).replace(".00", ""), small.clone(), p.muted);
            g += step;
        }
        let mut last_year = None;
        for (i, &d) in o.dates.iter().enumerate() {
            let year = DateTime::from_timestamp_millis(d).map(|d| d.format("%Y").to_string());
            if last_year.is_some() && year != last_year {
                painter.line_segment([Pos2::new(x(i), plot.bottom()), Pos2::new(x(i), plot.bottom() + 4.0)], Stroke::new(1.0, p.muted));
                painter.text(Pos2::new(x(i), plot.bottom() + 6.0), egui::Align2::CENTER_TOP, year.clone().unwrap_or_default(), small.clone(), p.muted);
            }
            last_year = year;
        }

        // Lines, benchmark first so the strategy draws on top.
        for i in (0..3).rev() {
            let pts: Vec<Pos2> = curves[i].iter().enumerate().map(|(k, v)| Pos2::new(x(k), y(v * START))).collect();
            painter.add(Shape::line(pts, Stroke::new(2.0, p.series[i])));
        }

        // Direct end labels, spread apart with leader lines when they'd collide.
        let mut ends: Vec<(usize, f32)> = (0..3).map(|i| (i, y(curves[i][n - 1] * START))).collect();
        ends.sort_by(|a, b| a.1.total_cmp(&b.1));
        let mut placed: Vec<f32> = Vec::new();
        for &(_, ey) in &ends {
            let min = placed.last().map_or(f32::MIN, |l| l + 30.0);
            placed.push(ey.max(min));
        }
        for (k, &(i, ey)) in ends.iter().enumerate() {
            let ly = placed[k];
            let from = Pos2::new(plot.right() + 3.0, ey);
            let to = Pos2::new(plot.right() + 14.0, ly);
            painter.line_segment([from, to], Stroke::new(1.0, p.series[i]));
            painter.text(Pos2::new(to.x + 4.0, ly - 7.0), egui::Align2::LEFT_CENTER, SERIES[i], small.clone(), p.ink);
            painter.text(Pos2::new(to.x + 4.0, ly + 7.0), egui::Align2::LEFT_CENTER, money(curves[i][n - 1] * START), theme::mono(12.0), p.muted);
        }

        // Hover: crosshair, ringed markers, and a tooltip with all three values.
        if let Some(pos) = resp.hover_pos().filter(|pos| pos.x >= plot.left() && pos.x <= plot.right()) {
            let k = (((pos.x - plot.left()) / plot.width()) * (n - 1) as f32).round().clamp(0.0, (n - 1) as f32) as usize;
            painter.line_segment([Pos2::new(x(k), plot.top()), Pos2::new(x(k), plot.bottom())], Stroke::new(1.0, p.muted));
            for (i, curve) in curves.iter().enumerate() {
                let c = Pos2::new(x(k), y(curve[k] * START));
                painter.circle(c, 4.0, p.series[i], Stroke::new(2.0, p.surface));
            }
            resp.on_hover_ui_at_pointer(|ui| {
                ui.label(RichText::new(date(o.dates[k])).font(theme::sans_semibold(theme::BODY)));
                for i in 0..3 {
                    ui.horizontal(|ui| {
                        let (r, _) = ui.allocate_exact_size(Vec2::new(12.0, 12.0), Sense::hover());
                        ui.painter().line_segment([r.left_center(), r.right_center()], Stroke::new(2.0, p.series[i]));
                        ui.label(RichText::new(SERIES[i]).color(p.ink));
                        ui.label(RichText::new(money(curves[i][k] * START)).font(theme::mono(12.5)).color(p.ink));
                    });
                }
            });
        }
    });
}

fn metrics_card(ui: &mut egui::Ui, o: &Outcome) {
    let p = pal(ui);
    const HEADERS: [(&str, &str); 9] = [
        ("", ""),
        ("Total", "Total return over the whole period"),
        ("Per year", "Compound annual growth rate"),
        ("Volatility", "Annualized standard deviation of weekly returns"),
        ("Sharpe", "Return above the T-bill rate per unit of volatility; higher is better"),
        ("Max drawdown", "Worst fall from a peak, the pain you'd have sat through"),
        ("Beta", "Sensitivity to SPY over the period"),
        ("Alpha · t", "Return beyond what beta explains vs. SPY, with its t-statistic (|t| ≥ 2 stands out from noise)"),
        ("Turnover", "Share of the portfolio traded per year; taxes and spreads come out of this"),
    ];
    theme::card(ui).inner_margin(Margin::symmetric(14, 10)).show(ui, |ui| {
        ui.label(RichText::new("Results").font(theme::sans_semibold(theme::TITLE)));
        ui.add_space(4.0);
        TableBuilder::new(ui)
            .id_salt("backtest_metrics")
            .striped(true)
            .vscroll(false)
            .cell_layout(egui::Layout::left_to_right(egui::Align::Center))
            .column(Column::exact(200.0))
            .columns(Column::initial(100.0), 8)
            .header(24.0, |mut h| {
                for (l, hint) in HEADERS {
                    h.col(|ui| {
                        let r = ui.label(theme::eyebrow(ui, l));
                        if !hint.is_empty() {
                            r.on_hover_text(hint);
                        }
                    });
                }
            })
            .body(|mut body| {
                let rows: [(&str, &Stats, Color32); 3] =
                    [(SERIES[0], &o.strategy.stats, p.series[0]), (SERIES[1], &o.equal_weight.stats, p.series[1]), (SERIES[2], &o.benchmark.stats, p.series[2])];
                for (name, s, color) in rows {
                    body.row(26.0, |mut row| {
                        let fig = |t: String| RichText::new(t).font(theme::mono(12.5)).color(p.ink);
                        row.col(|ui| {
                            let (r, _) = ui.allocate_exact_size(Vec2::new(14.0, 12.0), Sense::hover());
                            ui.painter().line_segment([r.left_center(), r.right_center()], Stroke::new(2.0, color));
                            ui.label(RichText::new(name).color(p.ink));
                        });
                        let cells = [
                            pct(s.total_return),
                            pct(s.cagr),
                            format!("{:.1}%", s.volatility * 100.0),
                            format!("{:.2}", s.sharpe),
                            format!("−{:.1}%", s.max_drawdown * 100.0),
                            s.fit.map(|f| format!("{:.2}", f.beta)).unwrap_or("1.00".into()),
                            s.fit.map(|f| format!("{} · {:.1}", pct(f.alpha), f.alpha_t)).unwrap_or("—".into()),
                            s.turnover.map(|t| format!("{:.0}%", t * 100.0)).unwrap_or("—".into()),
                        ];
                        for c in cells {
                            row.col(|ui| {
                                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                    ui.label(fig(c));
                                });
                            });
                        }
                    });
                }
            });
    });
}

fn caveats_card(ui: &mut egui::Ui, o: &Outcome) {
    let p = pal(ui);
    let years = o.dates.len() as f64 / 52.0;
    let notes = [
        ("Survivorship bias", "Only today's S&P 500 members are tested. Companies that failed or were dropped are missing, and ones added because they soared are in. Every line is flattered; compare the strategy with the equal-weight line, which shares the bias."),
        ("Momentum is flattered most", "Joining the index is partly a reward for a run-up, so the test can buy future members during the very run-up that got them in, years before you could have. Past winners that later collapsed out of the index never appear. Treat strong momentum results with the most suspicion."),
        ("Price-only", "Dividends aren't included in any line. Low-beta picks tend to be high-yield (utilities, telecoms, tobacco), so they look several percent a year worse than they were."),
        ("No value or quality", "Schwab only has today's P/E, ROE and margins. Using them on past dates would leak the future, so this tests momentum and low beta only."),
        ("Short sample", &*format!("About {years:.0} years, one stretch of market history. A few percent a year either way can be luck; check alpha's t-statistic.")),
        ("Overfitting", "Trying many mixes and keeping the best one finds luck, not skill. Pick a mix for a reason, then check it."),
        ("No costs or taxes", "Trades are free here. Selling winners at each rebalance has a real tax cost in a taxable account; see Turnover."),
    ];
    theme::card(ui).inner_margin(Margin::symmetric(16, 12)).show(ui, |ui| {
        ui.label(RichText::new("Read these before trusting the numbers").font(theme::sans_semibold(theme::TITLE)));
        ui.add_space(2.0);
        for (title, body) in notes {
            ui.horizontal_wrapped(|ui| {
                ui.spacing_mut().item_spacing.x = 6.0;
                ui.label(RichText::new(title).font(theme::sans_semibold(theme::BODY)).color(p.ink));
                ui.label(RichText::new(body).color(p.muted));
            });
        }
    });
}

fn rebalances_card(ui: &mut egui::Ui, o: &Outcome, names: &std::collections::HashMap<String, String>) {
    let p = pal(ui);
    theme::card(ui).inner_margin(Margin::symmetric(14, 8)).show(ui, |ui| {
        let id = ui.make_persistent_id("backtest_rebalances");
        egui::collapsing_header::CollapsingState::load_with_default_open(ui.ctx(), id, false)
            .show_header(ui, |ui| {
                ui.label(RichText::new("Rebalance history").font(theme::sans_semibold(theme::TITLE)));
                ui.label(RichText::new(format!("{} rebalances, newest first", o.rebalances.len())).color(p.muted));
            })
            .body_unindented(|ui| {
                for r in o.rebalances.iter().rev() {
                    ui.horizontal_wrapped(|ui| {
                        ui.spacing_mut().item_spacing.x = 6.0;
                        ui.label(RichText::new(date(r.date)).font(theme::mono_semibold(12.5)).color(p.ink));
                        ui.label(RichText::new(format!("traded {:.0}%", r.turnover * 100.0)).color(p.muted).font(theme::sans(theme::SMALL)));
                        for s in &r.picks {
                            let resp = egui::Frame::new()
                                .fill(p.neutral_soft)
                                .corner_radius(CornerRadius::same(4))
                                .inner_margin(Margin::symmetric(5, 1))
                                .show(ui, |ui| ui.label(RichText::new(s).font(theme::mono(12.0)).color(p.ink)))
                                .response;
                            if let Some(n) = names.get(s).filter(|n| !n.is_empty()) {
                                resp.on_hover_text(n);
                            }
                        }
                    });
                    ui.add_space(2.0);
                }
            });
    });
}
