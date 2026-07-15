use std::time::{Duration, Instant};

use ratatui::{
    layout::{Alignment, Constraint, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, BorderType, Borders, Cell, Clear, Paragraph, Row, Table, Tabs},
    Frame,
};

use super::app::{
    AccountSelectPhase, AccountSelectState, AmountEntryState, App, AppScreen, BlacklistFocus,
    BlacklistState, DeleteTarget, IndexLookupPhase, IndexLookupState, Mode, NewListFocus, NewListState,
    PlanReviewPhase, PlanReviewState, SubmitPhase, SubmitState,
};
use crate::accounts::Account;
use crate::orders::OrderOutcome;
use crate::rebalance::RebalancePlan;
use crate::registry::{holdings::CachedHoldings, RegistryEntry};

// ── Top-level render ───────────────────────────────────────────────────────────

pub fn render(frame: &mut Frame, app: &mut App) {
    let area = frame.area();
    match &mut app.screen {
        AppScreen::Watchlists => render_watchlists_screen(frame, app),
        AppScreen::IndexLookup(state) => render_index_lookup(frame, state, area),
        AppScreen::AccountSelect(state) => render_account_select(frame, state, area),
        AppScreen::AmountEntry(state) => render_amount_entry(frame, state, area),
        AppScreen::PlanReview(state) => render_plan_review(frame, state, area),
        AppScreen::Submitting(state) => render_submitting(frame, state, area),
    }
}

fn render_watchlists_screen(frame: &mut Frame, app: &mut App) {
    let area = frame.area();
    let chunks = Layout::vertical([
        Constraint::Length(3),
        Constraint::Min(0),
        Constraint::Length(1),
    ])
    .split(area);

    render_tabs(frame, app, chunks[0]);
    render_table(frame, app, chunks[1]);
    render_status(frame, app, chunks[2]);

    match &app.mode {
        Mode::AddSymbol(input) => render_add_symbol(frame, input.clone(), area),
        Mode::NewList(state)   => render_new_list(frame, state, area),
        Mode::ConfirmDelete(t) => render_confirm_delete(frame, t, app, area),
        Mode::Blacklist(state) => render_blacklist(frame, state, app, area),
        Mode::Detail           => render_detail(frame, app, area),
        Mode::Normal           => {}
    }
}

// ── Tabs ───────────────────────────────────────────────────────────────────────

fn render_tabs(frame: &mut Frame, app: &App, area: Rect) {
    if app.watchlists.is_empty() {
        let p = Paragraph::new(" No watchlists — press [n] to create one")
            .block(Block::default().borders(Borders::ALL).border_type(BorderType::Rounded));
        frame.render_widget(p, area);
        return;
    }

    let titles: Vec<Line> = app
        .watchlists
        .iter()
        .map(|wl| Line::from(format!(" {} ", wl.name)))
        .collect();

    let tabs = Tabs::new(titles)
        .select(app.active_tab)
        .block(Block::default().borders(Borders::ALL).border_type(BorderType::Rounded))
        .style(Style::default().fg(Color::DarkGray))
        .highlight_style(Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD))
        .divider("│");

    frame.render_widget(tabs, area);
}

// ── Quote table ────────────────────────────────────────────────────────────────

fn render_table(frame: &mut Frame, app: &mut App, area: Rect) {
    let symbols = app.current_symbols();

    if symbols.is_empty() {
        let msg = if app.watchlists.is_empty() {
            " Press [n] to create a watchlist."
        } else {
            " Empty watchlist — press [a] to add symbols."
        };
        frame.render_widget(
            Paragraph::new(msg)
                .block(Block::default().borders(Borders::ALL).border_type(BorderType::Rounded)),
            area,
        );
        return;
    }

    let header = Row::new(
        ["Symbol", "Company", "Last", "Chg", "Chg %", "Bid", "Ask", "Volume"]
            .iter()
            .map(|h| Cell::from(*h).style(Style::default().add_modifier(Modifier::BOLD).fg(Color::Cyan))),
    )
    .height(1);

    let now = Instant::now();
    let rows: Vec<Row> = symbols
        .iter()
        .map(|sym| {
            let q = app.quotes.get(sym);
            let fresh = q
                .and_then(|q| q.updated)
                .map(|u| now.duration_since(u) < Duration::from_millis(800))
                .unwrap_or(false);

            let chg = q.and_then(|q| q.change());
            let pct = q.and_then(|q| q.pct_change());
            let row_color = match chg {
                Some(c) if c > 0.0 => Color::Green,
                Some(c) if c < 0.0 => Color::Red,
                _ => Color::White,
            };
            let last_style = if fresh {
                Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(row_color)
            };

            let company = q
                .and_then(|q| q.description.as_deref())
                .map(|s| truncate(s, 20))
                .unwrap_or_default();

            Row::new(vec![
                Cell::from(sym.as_str())
                    .style(Style::default().fg(Color::White).add_modifier(Modifier::BOLD)),
                Cell::from(company).style(Style::default().fg(Color::DarkGray)),
                Cell::from(fmt_price(q.and_then(|q| q.last))).style(last_style),
                Cell::from(fmt_change(chg)).style(Style::default().fg(row_color)),
                Cell::from(fmt_pct(pct)).style(Style::default().fg(row_color)),
                Cell::from(fmt_price(q.and_then(|q| q.bid)))
                    .style(Style::default().fg(Color::DarkGray)),
                Cell::from(fmt_price(q.and_then(|q| q.ask)))
                    .style(Style::default().fg(Color::DarkGray)),
                Cell::from(fmt_volume(q.and_then(|q| q.volume)))
                    .style(Style::default().fg(Color::DarkGray)),
            ])
        })
        .collect();

    let widths = [
        Constraint::Length(8),
        Constraint::Length(21),
        Constraint::Length(9),
        Constraint::Length(9),
        Constraint::Length(9),
        Constraint::Length(9),
        Constraint::Length(9),
        Constraint::Min(7),
    ];

    let table = Table::new(rows, widths)
        .header(header)
        .block(Block::default().borders(Borders::ALL).border_type(BorderType::Rounded))
        .row_highlight_style(Style::default().add_modifier(Modifier::REVERSED))
        .highlight_symbol("▶ ");

    frame.render_stateful_widget(table, area, &mut app.table_state);
}

// ── Status bar ─────────────────────────────────────────────────────────────────

fn render_status(frame: &mut Frame, app: &App, area: Rect) {
    let ws = app.ws_status_text();
    let keys = " [a]dd [d]el [n]ew [D]el-list [b]lacklist [i]ndex [A]ccounts [↵] detail [h/l] tabs [PgUp/Dn] scroll [q]uit";
    let line = Line::from(vec![
        Span::styled(format!("  {}  ", ws), Style::default().fg(Color::Green)),
        Span::styled(keys, Style::default().fg(Color::DarkGray)),
    ]);
    frame.render_widget(Paragraph::new(line), area);
}

// ── Detail popup ───────────────────────────────────────────────────────────────

fn render_detail(frame: &mut Frame, app: &App, area: Rect) {
    let Some(sym) = app.selected_symbol() else { return };
    let q = app.quotes.get(sym);

    let title = match q.and_then(|q| q.description.as_deref()) {
        Some(d) => format!(" {sym} — {d} "),
        None    => format!(" {sym} "),
    };

    let popup = centered_rect(62, 70, area);
    frame.render_widget(Clear, popup);

    let block = Block::default()
        .title(title)
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(Color::Cyan));

    let inner = block.inner(popup);
    frame.render_widget(block, popup);

    let rows: Vec<(&str, String)> = vec![
        ("Last Price",  fmt_price(q.and_then(|q| q.last))),
        ("Prev Close",  fmt_price(q.and_then(|q| q.prev_close))),
        ("Change",      fmt_change(q.and_then(|q| q.change()))),
        ("Change %",    fmt_pct(q.and_then(|q| q.pct_change()))),
        ("",            String::new()),
        ("Open",        fmt_price(q.and_then(|q| q.open))),
        ("Day High",    fmt_price(q.and_then(|q| q.high))),
        ("Day Low",     fmt_price(q.and_then(|q| q.low))),
        ("Bid / Ask",   fmt_bid_ask(q)),
        ("Volume",      fmt_volume(q.and_then(|q| q.volume))),
        ("",            String::new()),
        ("52W High",    fmt_price(q.and_then(|q| q.w52_high))),
        ("52W Low",     fmt_price(q.and_then(|q| q.w52_low))),
        ("",            String::new()),
        ("EPS",         fmt_opt_f2(q.and_then(|q| q.eps))),
        ("PE Ratio",    fmt_opt_f2(q.and_then(|q| q.pe_ratio))),
        ("Div Yield",   fmt_div_yield(q.and_then(|q| q.div_yield))),
        ("Div Amount",  fmt_price(q.and_then(|q| q.div_amount))),
        ("Market Cap",  fmt_market_cap(q.and_then(|q| q.market_cap))),
    ];

    let chunks = Layout::vertical(
        std::iter::once(Constraint::Min(0))
            .chain(std::iter::once(Constraint::Length(1)))
            .collect::<Vec<_>>(),
    )
    .split(inner);

    let lines: Vec<Line> = rows
        .iter()
        .map(|(label, value)| {
            if label.is_empty() {
                Line::from("")
            } else {
                Line::from(vec![
                    Span::styled(format!("  {:<14}", label), Style::default().fg(Color::DarkGray)),
                    Span::styled(value.clone(), Style::default().fg(Color::White).add_modifier(Modifier::BOLD)),
                ])
            }
        })
        .collect();

    frame.render_widget(Paragraph::new(lines), chunks[0]);
    frame.render_widget(
        Paragraph::new("  [any key] close")
            .style(Style::default().fg(Color::DarkGray))
            .alignment(Alignment::Left),
        chunks[1],
    );
}

// ── Add symbol popup ───────────────────────────────────────────────────────────

fn render_add_symbol(frame: &mut Frame, input: String, area: Rect) {
    let popup = centered_rect(40, 20, area);
    frame.render_widget(Clear, popup);

    let block = Block::default()
        .title(" Add Symbol ")
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(Color::Yellow));

    let inner = block.inner(popup);
    frame.render_widget(block, popup);

    let chunks = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Length(1),
    ])
    .split(inner);

    frame.render_widget(
        Paragraph::new("Symbol:").style(Style::default().fg(Color::DarkGray)),
        chunks[0],
    );
    frame.render_widget(
        Paragraph::new(format!("{}_", input))
            .style(Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)),
        chunks[1],
    );
    frame.render_widget(
        Paragraph::new("[Enter] confirm  [Esc] cancel")
            .style(Style::default().fg(Color::DarkGray)),
        chunks[2],
    );
}

// ── New list popup ─────────────────────────────────────────────────────────────

fn render_new_list(frame: &mut Frame, state: &NewListState, area: Rect) {
    let popup = centered_rect(50, 60, area);
    frame.render_widget(Clear, popup);

    let block = Block::default()
        .title(" New Watchlist ")
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(Color::Yellow));

    let inner = block.inner(popup);
    frame.render_widget(block, popup);

    let src_rows = NewListState::sources().len() as u16 + 1;
    let chunks = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Length(src_rows),
        Constraint::Length(1),
    ])
    .split(inner);

    let name_label_style = if state.focus == NewListFocus::Name {
        Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(Color::DarkGray)
    };
    frame.render_widget(Paragraph::new("Name:").style(name_label_style), chunks[0]);
    frame.render_widget(
        Paragraph::new(format!("{}_", state.name)).style(Style::default().fg(Color::White)),
        chunks[1],
    );
    frame.render_widget(Paragraph::new(""), chunks[2]);

    let src_label_style = if state.focus == NewListFocus::Source {
        Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(Color::DarkGray)
    };
    let source_lines: Vec<Line> = std::iter::once(Line::from(Span::styled("Start from:", src_label_style)))
        .chain(NewListState::sources().iter().enumerate().map(|(i, &label)| {
            if i == state.source_idx {
                Line::from(Span::styled(
                    format!("▶   {}", label),
                    Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD),
                ))
            } else {
                Line::from(Span::styled(format!("    {}", label), Style::default().fg(Color::White)))
            }
        }))
        .collect();
    frame.render_widget(Paragraph::new(source_lines), chunks[3]);

    frame.render_widget(
        Paragraph::new("[Tab] toggle field  [↑↓] pick source  [Enter] create  [Esc] cancel")
            .style(Style::default().fg(Color::DarkGray))
            .alignment(Alignment::Center),
        chunks[4],
    );
}

// ── Confirm delete popup ───────────────────────────────────────────────────────

fn render_confirm_delete(frame: &mut Frame, target: &DeleteTarget, app: &App, area: Rect) {
    let popup = centered_rect(44, 22, area);
    frame.render_widget(Clear, popup);

    let label = match target {
        DeleteTarget::Symbol => {
            let sym = app.selected_symbol().unwrap_or("?");
            format!("Delete symbol {}?", sym)
        }
        DeleteTarget::List => {
            let name = app.watchlists.get(app.active_tab).map(|w| w.name.as_str()).unwrap_or("?");
            format!("Delete watchlist \"{}\"?", name)
        }
    };

    let block = Block::default()
        .title(" Confirm ")
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(Color::Red));
    let inner = block.inner(popup);
    frame.render_widget(block, popup);

    let chunks = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Length(1),
    ])
    .split(inner);
    frame.render_widget(Paragraph::new(label).alignment(Alignment::Center), chunks[0]);
    frame.render_widget(Paragraph::new(""), chunks[1]);
    frame.render_widget(
        Paragraph::new("[y] yes  [n / Esc] cancel")
            .alignment(Alignment::Center)
            .style(Style::default().fg(Color::DarkGray)),
        chunks[2],
    );
}

// ── Blacklist popup ────────────────────────────────────────────────────────────

fn render_blacklist(frame: &mut Frame, state: &BlacklistState, app: &App, area: Rect) {
    let popup = centered_rect(44, 60, area);
    frame.render_widget(Clear, popup);

    let block = Block::default()
        .title(" Blacklist — never trade ")
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(Color::Magenta));

    let inner = block.inner(popup);
    frame.render_widget(block, popup);

    let chunks = Layout::vertical([Constraint::Min(0), Constraint::Length(1)]).split(inner);

    if app.blacklist.is_empty() {
        frame.render_widget(
            Paragraph::new("  (empty) — press [a] to add a symbol")
                .style(Style::default().fg(Color::DarkGray)),
            chunks[0],
        );
    } else {
        let lines: Vec<Line> = app
            .blacklist
            .iter()
            .enumerate()
            .map(|(i, sym)| {
                if state.selected == Some(i) {
                    Line::from(Span::styled(
                        format!("▶ {}", sym),
                        Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD),
                    ))
                } else {
                    Line::from(Span::styled(format!("  {}", sym), Style::default().fg(Color::White)))
                }
            })
            .collect();
        frame.render_widget(Paragraph::new(lines), chunks[0]);
    }

    match &state.focus {
        BlacklistFocus::View => {
            frame.render_widget(
                Paragraph::new("[a]dd  [d]el  [Esc] close")
                    .style(Style::default().fg(Color::DarkGray)),
                chunks[1],
            );
        }
        BlacklistFocus::Add(input) => {
            frame.render_widget(
                Paragraph::new(format!("Add: {}_   [Enter] confirm  [Esc] cancel", input))
                    .style(Style::default().fg(Color::Yellow)),
                chunks[1],
            );
        }
        BlacklistFocus::ConfirmDelete => {
            let sym = state.selected.and_then(|i| app.blacklist.get(i)).map(String::as_str).unwrap_or("?");
            frame.render_widget(
                Paragraph::new(format!("Delete {}? [y]es  [n]o", sym))
                    .style(Style::default().fg(Color::Red)),
                chunks[1],
            );
        }
    }
}

// ── Index lookup screen ────────────────────────────────────────────────────────

fn render_index_lookup(frame: &mut Frame, state: &mut IndexLookupState, area: Rect) {
    let block = Block::default()
        .title(" Index Lookup ")
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(Color::Cyan));
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let chunks = Layout::vertical([Constraint::Length(2), Constraint::Min(0), Constraint::Length(1)]).split(inner);

    let query_line = Line::from(vec![
        Span::styled("Index / ETF ticker: ", Style::default().fg(Color::DarkGray)),
        Span::styled(format!("{}_", state.query), Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)),
    ]);
    frame.render_widget(Paragraph::new(query_line), chunks[0]);

    match &mut state.phase {
        IndexLookupPhase::Input => {
            frame.render_widget(
                Paragraph::new("Type an index/ETF ticker (e.g. SIXB, SPY, QQQ, XLK) and press Enter.")
                    .style(Style::default().fg(Color::DarkGray)),
                chunks[1],
            );
            frame.render_widget(
                Paragraph::new("[Enter] look up  [Esc] back").style(Style::default().fg(Color::DarkGray)),
                chunks[2],
            );
        }
        IndexLookupPhase::Loading => {
            frame.render_widget(
                Paragraph::new("Fetching holdings…").style(Style::default().fg(Color::Yellow)),
                chunks[1],
            );
            frame.render_widget(Paragraph::new("[Esc] cancel").style(Style::default().fg(Color::DarkGray)), chunks[2]);
        }
        IndexLookupPhase::NotFound(q) => {
            frame.render_widget(
                Paragraph::new(format!(
                    "No registry entry for \"{q}\". Known tickers: SPY, QQQ, DIA, XLB, XLC, XLE, XLF, XLI, XLK, XLP, XLRE, XLU, XLV, XLY."
                ))
                .style(Style::default().fg(Color::Red)),
                chunks[1],
            );
            frame.render_widget(
                Paragraph::new("[n]ew search  [Esc] back").style(Style::default().fg(Color::DarkGray)),
                chunks[2],
            );
        }
        IndexLookupPhase::Error(e) => {
            frame.render_widget(Paragraph::new(format!("Error: {e}")).style(Style::default().fg(Color::Red)), chunks[1]);
            frame.render_widget(
                Paragraph::new("[n]ew search  [Esc] back").style(Style::default().fg(Color::DarkGray)),
                chunks[2],
            );
        }
        IndexLookupPhase::Loaded { entry, holdings, table_state } => {
            render_holdings_table(frame, entry, holdings, table_state, chunks[1]);
            let hint = if holdings.fetch_error.is_some() {
                "showing stale cache, live refresh failed  [j/k] scroll  [p]lan investment  [n]ew search  [Esc] back"
            } else {
                "[j/k] scroll  [p]lan investment  [n]ew search  [Esc] back"
            };
            frame.render_widget(Paragraph::new(hint).style(Style::default().fg(Color::DarkGray)), chunks[2]);
        }
    }
}

fn render_holdings_table(
    frame: &mut Frame,
    entry: &RegistryEntry,
    holdings: &CachedHoldings,
    table_state: &mut ratatui::widgets::TableState,
    area: Rect,
) {
    let chunks = Layout::vertical([Constraint::Length(1), Constraint::Min(0)]).split(area);

    let as_of = holdings
        .source_as_of
        .clone()
        .unwrap_or_else(|| holdings.as_of.format("%Y-%m-%d").to_string());
    let header_line = Line::from(vec![
        Span::styled(format!("{} ", entry.label), Style::default().fg(Color::White).add_modifier(Modifier::BOLD)),
        Span::styled(
            format!("— {} constituents, tracking {}, as of {}", holdings.constituents.len(), entry.tracking_etf, as_of),
            Style::default().fg(Color::DarkGray),
        ),
    ]);
    frame.render_widget(Paragraph::new(header_line), chunks[0]);

    let header = Row::new(
        ["Symbol", "Weight"]
            .iter()
            .map(|h| Cell::from(*h).style(Style::default().add_modifier(Modifier::BOLD).fg(Color::Cyan))),
    )
    .height(1);

    let rows: Vec<Row> = holdings
        .constituents
        .iter()
        .map(|h| {
            Row::new(vec![
                Cell::from(h.symbol.clone()).style(Style::default().fg(Color::White).add_modifier(Modifier::BOLD)),
                Cell::from(format!("{:.2}%", h.weight * 100.0)).style(Style::default().fg(Color::DarkGray)),
            ])
        })
        .collect();

    let widths = [Constraint::Length(10), Constraint::Length(10)];
    let table = Table::new(rows, widths)
        .header(header)
        .block(Block::default().borders(Borders::ALL).border_type(BorderType::Rounded))
        .row_highlight_style(Style::default().add_modifier(Modifier::REVERSED))
        .highlight_symbol("▶ ");

    frame.render_stateful_widget(table, chunks[1], table_state);
}

// ── Account select screen ──────────────────────────────────────────────────────

fn render_account_select(frame: &mut Frame, state: &mut AccountSelectState, area: Rect) {
    let block = Block::default()
        .title(" Select Account ")
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(Color::Cyan));
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let chunks = Layout::vertical([Constraint::Min(0), Constraint::Length(1)]).split(inner);

    match &mut state.phase {
        AccountSelectPhase::Loading => {
            frame.render_widget(
                Paragraph::new("Fetching accounts…").style(Style::default().fg(Color::Yellow)),
                chunks[0],
            );
            frame.render_widget(Paragraph::new("[Esc] cancel").style(Style::default().fg(Color::DarkGray)), chunks[1]);
        }
        AccountSelectPhase::Error(e) => {
            frame.render_widget(Paragraph::new(format!("Error: {e}")).style(Style::default().fg(Color::Red)), chunks[0]);
            frame.render_widget(Paragraph::new("[Esc] back").style(Style::default().fg(Color::DarkGray)), chunks[1]);
        }
        AccountSelectPhase::Loaded { accounts, table_state } => {
            if accounts.is_empty() {
                frame.render_widget(
                    Paragraph::new("No linked accounts found.").style(Style::default().fg(Color::DarkGray)),
                    chunks[0],
                );
            } else {
                render_accounts_table(frame, accounts, table_state, chunks[0]);
            }
            frame.render_widget(
                Paragraph::new("[j/k] select  [Enter] confirm  [Esc] back").style(Style::default().fg(Color::DarkGray)),
                chunks[1],
            );
        }
    }
}

fn render_accounts_table(
    frame: &mut Frame,
    accounts: &[Account],
    table_state: &mut ratatui::widgets::TableState,
    area: Rect,
) {
    let header = Row::new(
        ["Account", "Type", "Cash", "Liquidation Value", "Positions"]
            .iter()
            .map(|h| Cell::from(*h).style(Style::default().add_modifier(Modifier::BOLD).fg(Color::Cyan))),
    )
    .height(1);

    let rows: Vec<Row> = accounts
        .iter()
        .map(|a| {
            Row::new(vec![
                Cell::from(mask_account_number(&a.account_number)).style(Style::default().fg(Color::White).add_modifier(Modifier::BOLD)),
                Cell::from(a.account_type.clone()).style(Style::default().fg(Color::DarkGray)),
                Cell::from(fmt_price(Some(a.cash_balance))).style(Style::default().fg(Color::DarkGray)),
                Cell::from(fmt_price(Some(a.liquidation_value))).style(Style::default().fg(Color::White)),
                Cell::from(a.positions.len().to_string()).style(Style::default().fg(Color::DarkGray)),
            ])
        })
        .collect();

    let widths = [
        Constraint::Length(12),
        Constraint::Length(10),
        Constraint::Length(14),
        Constraint::Length(18),
        Constraint::Length(10),
    ];

    let table = Table::new(rows, widths)
        .header(header)
        .block(Block::default().borders(Borders::ALL).border_type(BorderType::Rounded))
        .row_highlight_style(Style::default().add_modifier(Modifier::REVERSED))
        .highlight_symbol("▶ ");

    frame.render_stateful_widget(table, area, table_state);
}

/// Show only the last 4 digits — account numbers shouldn't be fully visible
/// on screen (screenshots, screen-share, shoulder-surfing).
fn mask_account_number(n: &str) -> String {
    if n.len() <= 4 { format!("...{n}") } else { format!("...{}", &n[n.len() - 4..]) }
}

// ── Amount entry screen ────────────────────────────────────────────────────────

fn render_amount_entry(frame: &mut Frame, state: &AmountEntryState, area: Rect) {
    let block = Block::default()
        .title(format!(" Invest in {} ", state.entry.label))
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(Color::Cyan));
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let chunks = Layout::vertical([
        Constraint::Length(2),
        Constraint::Length(2),
        Constraint::Length(1),
        Constraint::Min(0),
        Constraint::Length(1),
    ])
    .split(inner);

    let info = format!(
        "{} constituents, tracking {}",
        state.holdings.constituents.len(),
        state.entry.tracking_etf
    );
    frame.render_widget(Paragraph::new(info).style(Style::default().fg(Color::DarkGray)), chunks[0]);

    let amount_line = Line::from(vec![
        Span::styled("Amount to invest: $", Style::default().fg(Color::DarkGray)),
        Span::styled(
            format!("{}_", state.amount_input),
            Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD),
        ),
    ]);
    frame.render_widget(Paragraph::new(amount_line), chunks[1]);

    if let Some(err) = &state.error {
        frame.render_widget(Paragraph::new(err.as_str()).style(Style::default().fg(Color::Red)), chunks[2]);
    }

    frame.render_widget(
        Paragraph::new("[Enter] build plan  [Esc] back").style(Style::default().fg(Color::DarkGray)),
        chunks[4],
    );
}

// ── Plan review screen ─────────────────────────────────────────────────────────

fn render_plan_review(frame: &mut Frame, state: &mut PlanReviewState, area: Rect) {
    let block = Block::default()
        .title(" Investment Plan ")
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(Color::Cyan));
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let chunks = Layout::vertical([Constraint::Min(0), Constraint::Length(1)]).split(inner);

    match &mut state.phase {
        PlanReviewPhase::Loading => {
            frame.render_widget(
                Paragraph::new("Fetching prices and building plan…").style(Style::default().fg(Color::Yellow)),
                chunks[0],
            );
            frame.render_widget(Paragraph::new("[Esc] cancel").style(Style::default().fg(Color::DarkGray)), chunks[1]);
        }
        PlanReviewPhase::Error(e) => {
            frame.render_widget(Paragraph::new(format!("Error: {e}")).style(Style::default().fg(Color::Red)), chunks[0]);
            frame.render_widget(Paragraph::new("[Esc] back").style(Style::default().fg(Color::DarkGray)), chunks[1]);
        }
        PlanReviewPhase::Loaded { plan, table_state } => {
            render_plan_table(frame, plan, table_state, chunks[0]);
            frame.render_widget(
                Paragraph::new("[j/k] scroll  [s] dry-run submit  [Esc] back").style(Style::default().fg(Color::DarkGray)),
                chunks[1],
            );
        }
    }
}

fn render_plan_table(
    frame: &mut Frame,
    plan: &RebalancePlan,
    table_state: &mut ratatui::widgets::TableState,
    area: Rect,
) {
    let chunks = Layout::vertical([Constraint::Length(3), Constraint::Min(0)]).split(area);

    let mut summary_lines = vec![Line::from(vec![
        Span::styled(format!("{} ", plan.index_name), Style::default().fg(Color::White).add_modifier(Modifier::BOLD)),
        Span::styled(
            format!(
                "— investing {}, holdings as of {}",
                fmt_price(Some(plan.total_amount)),
                plan.as_of_holdings.format("%Y-%m-%d %H:%M UTC")
            ),
            Style::default().fg(Color::DarkGray),
        ),
    ])];
    if !plan.blacklisted_excluded.is_empty() {
        summary_lines.push(Line::from(Span::styled(
            format!("Blacklisted (excluded): {}", plan.blacklisted_excluded.join(", ")),
            Style::default().fg(Color::DarkGray),
        )));
    }
    if !plan.price_unavailable.is_empty() || plan.cash_remainder.abs() > 0.01 {
        let extra = if plan.price_unavailable.is_empty() {
            String::new()
        } else {
            format!(" (no price for: {})", plan.price_unavailable.join(", "))
        };
        summary_lines.push(Line::from(Span::styled(
            format!("Unallocated cash: {}{}", fmt_price(Some(plan.cash_remainder)), extra),
            Style::default().fg(Color::Yellow),
        )));
    }
    frame.render_widget(Paragraph::new(summary_lines), chunks[0]);

    let header = Row::new(
        ["Symbol", "Weight", "Price", "Target $", "Shares", "Whole Shares"]
            .iter()
            .map(|h| Cell::from(*h).style(Style::default().add_modifier(Modifier::BOLD).fg(Color::Cyan))),
    )
    .height(1);

    let rows: Vec<Row> = plan
        .lines
        .iter()
        .map(|l| {
            Row::new(vec![
                Cell::from(l.symbol.clone()).style(Style::default().fg(Color::White).add_modifier(Modifier::BOLD)),
                Cell::from(format!("{:.2}%", l.target_weight * 100.0)).style(Style::default().fg(Color::DarkGray)),
                Cell::from(fmt_price(Some(l.last_price))).style(Style::default().fg(Color::DarkGray)),
                Cell::from(fmt_price(Some(l.target_dollars))).style(Style::default().fg(Color::White)),
                Cell::from(format!("{:.4}", l.target_shares)).style(Style::default().fg(Color::White)),
                Cell::from(l.whole_shares_fallback.to_string()).style(Style::default().fg(Color::DarkGray)),
            ])
        })
        .collect();

    let widths = [
        Constraint::Length(8),
        Constraint::Length(8),
        Constraint::Length(10),
        Constraint::Length(12),
        Constraint::Length(10),
        Constraint::Length(12),
    ];

    let table = Table::new(rows, widths)
        .header(header)
        .block(Block::default().borders(Borders::ALL).border_type(BorderType::Rounded))
        .row_highlight_style(Style::default().add_modifier(Modifier::REVERSED))
        .highlight_symbol("▶ ");

    frame.render_stateful_widget(table, chunks[1], table_state);
}

// ── Submitting screen ──────────────────────────────────────────────────────────

fn render_submitting(frame: &mut Frame, state: &mut SubmitState, area: Rect) {
    let block = Block::default()
        .title(" Order Submission ")
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(Color::Cyan));
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let chunks = Layout::vertical([Constraint::Min(0), Constraint::Length(1)]).split(inner);

    match &mut state.phase {
        SubmitPhase::Submitting => {
            frame.render_widget(
                Paragraph::new("Submitting (dry run — nothing is actually sent to Schwab)…")
                    .style(Style::default().fg(Color::Yellow)),
                chunks[0],
            );
            frame.render_widget(Paragraph::new("[Esc] back").style(Style::default().fg(Color::DarkGray)), chunks[1]);
        }
        SubmitPhase::Done { results, table_state, viewing } => {
            let header = Row::new(
                ["Symbol", "Qty", "Outcome"]
                    .iter()
                    .map(|h| Cell::from(*h).style(Style::default().add_modifier(Modifier::BOLD).fg(Color::Cyan))),
            )
            .height(1);

            let rows: Vec<Row> = results
                .iter()
                .map(|r| {
                    let (label, color) = match &r.outcome {
                        OrderOutcome::DryRun { .. } => ("DRY RUN — not sent".to_string(), Color::Yellow),
                        OrderOutcome::Submitted { .. } => ("Submitted".to_string(), Color::Green),
                        OrderOutcome::Rejected { status, body } => {
                            (format!("Rejected ({status}): {}", truncate(body, 40)), Color::Red)
                        }
                    };
                    Row::new(vec![
                        Cell::from(r.symbol.clone()).style(Style::default().fg(Color::White).add_modifier(Modifier::BOLD)),
                        Cell::from(format!("{:.4}", r.quantity)).style(Style::default().fg(Color::DarkGray)),
                        Cell::from(label).style(Style::default().fg(color)),
                    ])
                })
                .collect();

            let widths = [Constraint::Length(8), Constraint::Length(10), Constraint::Min(20)];
            let table = Table::new(rows, widths)
                .header(header)
                .block(Block::default().borders(Borders::ALL).border_type(BorderType::Rounded))
                .row_highlight_style(Style::default().add_modifier(Modifier::REVERSED))
                .highlight_symbol("▶ ");
            frame.render_stateful_widget(table, chunks[0], table_state);
            frame.render_widget(
                Paragraph::new("[j/k] select  [Enter/v] view request  [Esc] back").style(Style::default().fg(Color::DarkGray)),
                chunks[1],
            );

            if let Some(idx) = *viewing {
                if let Some(r) = results.get(idx) {
                    render_order_detail(frame, r, area);
                }
            }
        }
    }
}

fn render_order_detail(frame: &mut Frame, r: &crate::orders::OrderResult, area: Rect) {
    let popup = centered_rect(70, 60, area);
    frame.render_widget(Clear, popup);

    let block = Block::default()
        .title(format!(" {} — request detail ", r.symbol))
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(Color::Yellow));
    let inner = block.inner(popup);
    frame.render_widget(block, popup);

    let text = match &r.outcome {
        OrderOutcome::DryRun { request_body } => {
            serde_json::to_string_pretty(request_body).unwrap_or_else(|_| "(failed to render JSON)".to_string())
        }
        OrderOutcome::Submitted { order_location } => {
            format!("Submitted.\nOrder location: {}", order_location.as_deref().unwrap_or("(none returned)"))
        }
        OrderOutcome::Rejected { status, body } => format!("Rejected (HTTP {status}):\n{body}"),
    };

    let chunks = Layout::vertical([Constraint::Min(0), Constraint::Length(1)]).split(inner);
    frame.render_widget(Paragraph::new(text).style(Style::default().fg(Color::White)), chunks[0]);
    frame.render_widget(
        Paragraph::new("[Esc/Enter/v] close").style(Style::default().fg(Color::DarkGray)),
        chunks[1],
    );
}

// ── Formatting helpers ─────────────────────────────────────────────────────────

fn fmt_price(v: Option<f64>) -> String {
    v.map(|n| format!("${:.2}", n)).unwrap_or_else(|| "—".to_string())
}

fn fmt_change(v: Option<f64>) -> String {
    v.map(|n| if n >= 0.0 { format!("+{:.2}", n) } else { format!("{:.2}", n) })
        .unwrap_or_else(|| "—".to_string())
}

fn fmt_pct(v: Option<f64>) -> String {
    v.map(|n| if n >= 0.0 { format!("+{:.2}%", n) } else { format!("{:.2}%", n) })
        .unwrap_or_else(|| "—".to_string())
}

fn fmt_volume(v: Option<u64>) -> String {
    match v {
        None => "—".to_string(),
        Some(n) if n >= 1_000_000 => format!("{:.1}M", n as f64 / 1_000_000.0),
        Some(n) if n >= 1_000     => format!("{:.1}K", n as f64 / 1_000.0),
        Some(n) => n.to_string(),
    }
}

fn fmt_opt_f2(v: Option<f64>) -> String {
    v.map(|n| format!("{:.2}", n)).unwrap_or_else(|| "—".to_string())
}

fn fmt_div_yield(v: Option<f64>) -> String {
    // Schwab returns div yield as a percentage already (e.g. 2.1 means 2.1%)
    v.map(|n| format!("{:.2}%", n)).unwrap_or_else(|| "—".to_string())
}

fn fmt_market_cap(v: Option<f64>) -> String {
    match v {
        None => "—".to_string(),
        Some(n) if n >= 1e12 => format!("${:.2}T", n / 1e12),
        Some(n) if n >= 1e9  => format!("${:.2}B", n / 1e9),
        Some(n) if n >= 1e6  => format!("${:.2}M", n / 1e6),
        Some(n) => format!("${:.0}", n),
    }
}

fn fmt_bid_ask(q: Option<&super::app::Quote>) -> String {
    match q {
        None => "—".to_string(),
        Some(q) => match (q.bid, q.ask) {
            (Some(b), Some(a)) => format!("${:.2} / ${:.2}", b, a),
            _ => "—".to_string(),
        },
    }
}

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        format!("{}…", s.chars().take(max - 1).collect::<String>())
    }
}

// ── Layout utility ─────────────────────────────────────────────────────────────

fn centered_rect(pct_x: u16, pct_y: u16, area: Rect) -> Rect {
    let vert = Layout::vertical([
        Constraint::Percentage((100 - pct_y) / 2),
        Constraint::Percentage(pct_y),
        Constraint::Percentage((100 - pct_y) / 2),
    ])
    .split(area);

    Layout::horizontal([
        Constraint::Percentage((100 - pct_x) / 2),
        Constraint::Percentage(pct_x),
        Constraint::Percentage((100 - pct_x) / 2),
    ])
    .split(vert[1])[1]
}
