use gulfwatch_core::alert::AlertEvent;
use gulfwatch_core::cu_attribution::NATIVE_PROGRAM_CU;
use gulfwatch_core::transaction::Transaction;
use ratatui::{
    prelude::*,
    widgets::{
        Block, Borders, Cell, Paragraph, Row, Scrollbar, ScrollbarOrientation, ScrollbarState,
        Table, Wrap,
    },
};

use crate::app::{
    short_program_id, App, View, PANEL_ALERTS, PANEL_METRICS, PANEL_SIDEBAR, PANEL_TRANSACTIONS,
};

const ACTIVE_BORDER: Color = Color::Cyan;
const INACTIVE_BORDER: Color = Color::DarkGray;
const SUCCESS_COLOR: Color = Color::Green;
const ERROR_COLOR: Color = Color::Red;
const ALERT_COLOR: Color = Color::Yellow;
const HEADER_COLOR: Color = Color::Cyan;
const DIM_COLOR: Color = Color::DarkGray;
const SELECTED_BG: Color = Color::Rgb(30, 40, 60);

pub fn draw(f: &mut Frame, app: &App) {
    match &app.view {
        View::Dashboard => draw_dashboard(f, app),
        View::TransactionDetail(tx) => draw_tx_detail(f, app, tx),
        View::AlertDetail(alert) => draw_alert_detail(f, app, alert),
    }
}

// ─── Dashboard ───────────────────────────────────────────

fn draw_dashboard(f: &mut Frame, app: &App) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Min(0),
            Constraint::Length(1),
        ])
        .split(f.area());

    draw_header(f, chunks[0]);
    draw_main(f, chunks[1], app);
    draw_status_bar(f, chunks[2], app);
}

fn draw_header(f: &mut Frame, area: Rect) {
    let header = Paragraph::new(Line::from(vec![
        Span::styled(" GULF", Style::default().fg(Color::Cyan).bold()),
        Span::styled("WATCH ", Style::default().fg(Color::White).bold()),
        Span::styled("│ ", Style::default().fg(DIM_COLOR)),
        Span::styled(
            "Solana Program Observability",
            Style::default().fg(DIM_COLOR),
        ),
    ]))
    .block(
        Block::default()
            .borders(Borders::BOTTOM)
            .border_style(Style::default().fg(DIM_COLOR)),
    );
    f.render_widget(header, area);
}

fn draw_main(f: &mut Frame, area: Rect, app: &App) {
    let h_chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Length(24),
            Constraint::Percentage(55),
            Constraint::Min(30),
        ])
        .split(area);

    let right_chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Percentage(60), Constraint::Percentage(40)])
        .split(h_chunks[2]);

    draw_sidebar(f, h_chunks[0], app);
    draw_transactions(f, h_chunks[1], app);
    draw_metrics(f, right_chunks[0], app);
    draw_alerts(f, right_chunks[1], app);
}

fn draw_sidebar(f: &mut Frame, area: Rect, app: &App) {
    let block = panel_border("Programs [0]", PANEL_SIDEBAR, app);
    let inner = block.inner(area);
    f.render_widget(block, area);

    let is_active = app.active_panel == PANEL_SIDEBAR;
    let mut lines: Vec<Line> = Vec::new();

    let all_focused = app.selected_program.is_none();
    let all_selected = is_active && app.selected == 0;
    let all_count: usize = app.transactions.len();
    let all_tag = if all_focused { "▸" } else { " " };
    let all_style = if all_selected {
        Style::default().bg(SELECTED_BG).fg(Color::White).bold()
    } else if all_focused {
        Style::default().fg(HEADER_COLOR).bold()
    } else {
        Style::default().fg(Color::White)
    };
    lines.push(Line::styled(
        format!(" {} All  {:>5}", all_tag, all_count),
        all_style,
    ));
    lines.push(Line::styled(
        "".to_string(),
        Style::default().fg(DIM_COLOR),
    ));

    let windows = app.state.windows.try_read().ok();

    for (i, pid) in app.programs.iter().enumerate() {
        let row_idx = i + 1; // row 0 is All
        let focused = app.selected_program == Some(i);
        let selected = is_active && app.selected == row_idx;
        let has_alert = app.program_has_recent_alert(pid);

        let (tx_count, error_count) = match windows.as_ref().and_then(|w| w.get(pid)) {
            Some(window) => {
                let s = window.summary(pid);
                (s.tx_count, s.error_count)
            }
            None => {
                let count = app
                    .transactions
                    .iter()
                    .filter(|t| &t.program_id == pid)
                    .count() as u64;
                (count, 0u64)
            }
        };

        let cursor = if focused { "▸" } else { " " };
        let flag = if has_alert { "⚑" } else { " " };
        let flag_color = if has_alert { ALERT_COLOR } else { DIM_COLOR };

        let row_style = if selected {
            Style::default().bg(SELECTED_BG).fg(Color::White).bold()
        } else if focused {
            Style::default().fg(HEADER_COLOR).bold()
        } else {
            Style::default().fg(Color::White)
        };

        lines.push(Line::from(vec![
            Span::styled(format!(" {} ", cursor), row_style),
            Span::styled(short_program_id(pid), row_style),
            Span::styled(format!(" {:>4}", tx_count), row_style),
            Span::styled(" ", row_style),
            Span::styled(flag.to_string(), Style::default().fg(flag_color).bold()),
        ]));

        if error_count > 0 {
            lines.push(Line::from(vec![
                Span::styled("     ", Style::default()),
                Span::styled(
                    format!("{} err", error_count),
                    Style::default().fg(ERROR_COLOR),
                ),
            ]));
        }
    }

    if app.programs.is_empty() {
        lines.push(Line::from(Span::styled(
            " (none yet)",
            Style::default().fg(DIM_COLOR),
        )));
    }

    f.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), inner);
}

fn panel_border<'a>(title: &'a str, panel_idx: usize, app: &'a App) -> Block<'a> {
    let is_active = app.active_panel == panel_idx;
    let border_color = if is_active {
        ACTIVE_BORDER
    } else {
        INACTIVE_BORDER
    };

    Block::default()
        .title(Line::from(vec![Span::styled(
            format!(" {} ", title),
            Style::default().fg(border_color).bold(),
        )]))
        .borders(Borders::ALL)
        .border_style(Style::default().fg(border_color))
}

fn draw_transactions(f: &mut Frame, area: Rect, app: &App) {
    let title = scoped_title("Transactions [1]", app);
    let block = panel_border(&title, PANEL_TRANSACTIONS, app);
    let inner = block.inner(area);
    f.render_widget(block, area);

    let filtered: Vec<&Transaction> = app.filtered_transactions().collect();

    if filtered.is_empty() {
        let msg = Paragraph::new("Waiting for transactions...")
            .style(Style::default().fg(DIM_COLOR))
            .alignment(Alignment::Center);
        f.render_widget(msg, inner);
        return;
    }

    let header = Row::new(vec![
        Cell::from("Sig").style(Style::default().fg(HEADER_COLOR)),
        Cell::from("").style(Style::default().fg(HEADER_COLOR)),
        Cell::from("Program").style(Style::default().fg(HEADER_COLOR)),
        Cell::from("Type").style(Style::default().fg(HEADER_COLOR)),
        Cell::from("CU").style(Style::default().fg(HEADER_COLOR)),
        Cell::from("Fee").style(Style::default().fg(HEADER_COLOR)),
        Cell::from("Time").style(Style::default().fg(HEADER_COLOR)),
    ])
    .height(1);

    let is_active = app.active_panel == PANEL_TRANSACTIONS;
    let visible_rows = inner.height.saturating_sub(1) as usize; // minus header

    // Ensure selected is visible by computing scroll offset
    let scroll = if is_active {
        if app.selected >= visible_rows {
            app.selected - visible_rows + 1
        } else {
            0
        }
    } else {
        0
    };

    let rows: Vec<Row> = filtered
        .iter()
        .enumerate()
        .skip(scroll)
        .take(visible_rows)
        .map(|(i, tx)| {
            let status_style = if tx.success {
                Style::default().fg(SUCCESS_COLOR)
            } else {
                Style::default().fg(ERROR_COLOR)
            };

            let sig_short = if tx.signature.len() > 10 {
                format!("{}…", &tx.signature[..10])
            } else {
                tx.signature.clone()
            };

            let time = tx.timestamp.format("%H:%M:%S").to_string();
            let tx_type = tx
                .classification
                .as_ref()
                .and_then(|c| {
                    if c.classifier == "fallback" {
                        tx.instruction_type.clone().or_else(|| {
                            if c.category == "other" {
                                None
                            } else {
                                Some(c.category.clone())
                            }
                        })
                    } else {
                        Some(c.classifier.clone())
                    }
                })
                .or_else(|| tx.instruction_type.clone())
                .unwrap_or_else(|| "—".to_string());

            let mut row = Row::new(vec![
                Cell::from(sig_short),
                Cell::from(if tx.success { "✓" } else { "✗" }).style(status_style),
                Cell::from(short_program_id(&tx.program_id))
                    .style(Style::default().fg(HEADER_COLOR)),
                Cell::from(tx_type),
                Cell::from(format_cu(tx.compute_units)),
                Cell::from(format!("{}◎", tx.fee_lamports)),
                Cell::from(time).style(Style::default().fg(DIM_COLOR)),
            ]);

            if is_active && i == app.selected {
                row = row.style(Style::default().bg(SELECTED_BG).fg(Color::White));
            }

            row
        })
        .collect();

    let widths = [
        Constraint::Length(12),
        Constraint::Length(2),
        Constraint::Length(12),
        Constraint::Length(10),
        Constraint::Length(8),
        Constraint::Length(10),
        Constraint::Length(8),
    ];

    let table = Table::new(rows, widths).header(header);
    f.render_widget(table, inner);

    // Scrollbar
    if is_active && filtered.len() > visible_rows {
        let mut scrollbar_state = ScrollbarState::new(filtered.len()).position(app.selected);
        f.render_stateful_widget(
            Scrollbar::new(ScrollbarOrientation::VerticalRight),
            area,
            &mut scrollbar_state,
        );
    }
}

fn scoped_title(base: &str, app: &App) -> String {
    match app.selected_program {
        Some(_) => format!("{} [{}]", base, app.focused_program_label()),
        None => base.to_string(),
    }
}

fn draw_metrics(f: &mut Frame, area: Rect, app: &App) {
    let title = scoped_title("Metrics [2]", app);
    let block = panel_border(&title, PANEL_METRICS, app);
    let inner = block.inner(area);
    f.render_widget(block, area);

    let windows = match app.state.windows.try_read() {
        Ok(w) => w,
        Err(_) => {
            f.render_widget(
                Paragraph::new("Loading...").style(Style::default().fg(DIM_COLOR)),
                inner,
            );
            return;
        }
    };

    if app.programs.is_empty() {
        f.render_widget(
            Paragraph::new("No programs monitored")
                .style(Style::default().fg(DIM_COLOR))
                .alignment(Alignment::Center),
            inner,
        );
        return;
    }

    let focused = app.focused_program_id().map(|s| s.to_string());
    let program_iter: Vec<&String> = match &focused {
        Some(pid) => app.programs.iter().filter(|p| *p == pid).collect(),
        None => app.programs.iter().collect(),
    };

    let mut lines: Vec<Line> = Vec::new();

    for pid in program_iter {
        if let Some(window) = windows.get(pid) {
            let summary = window.summary(pid);

            let pid_short = if pid.len() > 16 {
                format!("{}…{}", &pid[..8], &pid[pid.len() - 4..])
            } else {
                pid.clone()
            };

            lines.push(Line::from(vec![Span::styled(
                format!("▸ {} ", pid_short),
                Style::default().fg(HEADER_COLOR).bold(),
            )]));

            lines.push(Line::from(vec![
                Span::styled("  Transactions: ", Style::default().fg(DIM_COLOR)),
                Span::styled(
                    format!("{}", summary.tx_count),
                    Style::default().fg(Color::White),
                ),
            ]));

            let err_color = if summary.error_rate > 0.1 {
                ERROR_COLOR
            } else if summary.error_rate > 0.0 {
                ALERT_COLOR
            } else {
                SUCCESS_COLOR
            };

            lines.push(Line::from(vec![
                Span::styled("  Error Rate:   ", Style::default().fg(DIM_COLOR)),
                Span::styled(
                    format!("{:.1}%", summary.error_rate * 100.0),
                    Style::default().fg(err_color),
                ),
                Span::styled(
                    format!(" ({} errors)", summary.error_count),
                    Style::default().fg(DIM_COLOR),
                ),
            ]));

            lines.push(Line::from(vec![
                Span::styled("  Avg CU:       ", Style::default().fg(DIM_COLOR)),
                Span::styled(
                    format_cu(summary.avg_compute_units as u64),
                    Style::default().fg(Color::White),
                ),
            ]));

            if !summary.top_instructions.is_empty() {
                lines.push(Line::from(Span::styled(
                    "  Top Instrs:",
                    Style::default().fg(DIM_COLOR),
                )));
                for instr in summary.top_instructions.iter().take(5) {
                    lines.push(Line::from(vec![
                        Span::styled(
                            format!("    {} ", instr.instruction_type),
                            Style::default().fg(Color::White),
                        ),
                        Span::styled(format!("({})", instr.count), Style::default().fg(DIM_COLOR)),
                    ]));
                }
            }

            lines.push(Line::from(""));
        }
    }

    let visible = inner.height as usize;
    let content_len = lines.len();
    let max_scroll = content_len.saturating_sub(visible) as u16;
    let scroll_offset = app.metrics_scroll.min(max_scroll);

    let is_active = app.active_panel == PANEL_METRICS;

    f.render_widget(
        Paragraph::new(lines)
            .wrap(Wrap { trim: false })
            .scroll((scroll_offset, 0)),
        inner,
    );

    if is_active && content_len > visible {
        let mut scrollbar_state =
            ScrollbarState::new(content_len).position(scroll_offset as usize);
        f.render_stateful_widget(
            Scrollbar::new(ScrollbarOrientation::VerticalRight),
            area,
            &mut scrollbar_state,
        );
    }
}

fn draw_alerts(f: &mut Frame, area: Rect, app: &App) {
    let filtered: Vec<&AlertEvent> = app.filtered_alerts().collect();
    let count = filtered.len();
    let base = if count > 0 {
        format!("Alerts [3] ({})", count)
    } else {
        "Alerts [3]".to_string()
    };
    let title = scoped_title(&base, app);

    let block = panel_border(&title, PANEL_ALERTS, app);
    let inner = block.inner(area);
    f.render_widget(block, area);

    if filtered.is_empty() {
        f.render_widget(
            Paragraph::new("No alerts")
                .style(Style::default().fg(DIM_COLOR))
                .alignment(Alignment::Center),
            inner,
        );
        return;
    }

    let is_active = app.active_panel == PANEL_ALERTS;
    let visible_rows = inner.height as usize;
    let scroll = if is_active && app.selected >= visible_rows {
        app.selected - visible_rows + 1
    } else {
        0
    };

    let lines: Vec<Line> = filtered
        .iter()
        .enumerate()
        .skip(scroll)
        .take(visible_rows)
        .map(|(i, alert)| {
            let time = alert.fired_at.format("%H:%M:%S");
            let mut style = Style::default();
            if is_active && i == app.selected {
                style = style.bg(SELECTED_BG);
            }

            Line::styled(
                format!(
                    " {} ⚑ {}  ({}: {:.2} > {})",
                    time, alert.rule_name, alert.metric, alert.value, alert.threshold
                ),
                style,
            )
        })
        .collect();

    f.render_widget(Paragraph::new(lines), inner);
}

fn draw_status_bar(f: &mut Frame, area: Rect, app: &App) {
    let panel_names = ["Programs", "Transactions", "Metrics", "Alerts"];
    let active = panel_names[app.active_panel];
    let focus_label = app.focused_program_label();

    let bar = Paragraph::new(Line::from(vec![
        Span::styled(" q", Style::default().fg(HEADER_COLOR).bold()),
        Span::styled(" quit  ", Style::default().fg(DIM_COLOR)),
        Span::styled("Tab", Style::default().fg(HEADER_COLOR).bold()),
        Span::styled(" switch  ", Style::default().fg(DIM_COLOR)),
        Span::styled("1-9", Style::default().fg(HEADER_COLOR).bold()),
        Span::styled(" program  ", Style::default().fg(DIM_COLOR)),
        Span::styled("a", Style::default().fg(HEADER_COLOR).bold()),
        Span::styled(" all  ", Style::default().fg(DIM_COLOR)),
        Span::styled("Enter", Style::default().fg(HEADER_COLOR).bold()),
        Span::styled(" detail  ", Style::default().fg(DIM_COLOR)),
        Span::styled("Esc", Style::default().fg(HEADER_COLOR).bold()),
        Span::styled(" back  ", Style::default().fg(DIM_COLOR)),
        Span::styled("│ ", Style::default().fg(DIM_COLOR)),
        Span::styled(format!("{} ", active), Style::default().fg(Color::White)),
        Span::styled("│ focus ", Style::default().fg(DIM_COLOR)),
        Span::styled(
            format!("{} ", focus_label),
            Style::default().fg(HEADER_COLOR),
        ),
        Span::styled("│ ", Style::default().fg(DIM_COLOR)),
        Span::styled(
            format!("{} txs ", app.transactions.len()),
            Style::default().fg(Color::White),
        ),
        Span::styled(
            format!("{} alerts", app.alerts.len()),
            Style::default().fg(if app.alerts.is_empty() {
                DIM_COLOR
            } else {
                ALERT_COLOR
            }),
        ),
    ]))
    .style(Style::default().bg(Color::DarkGray));

    f.render_widget(bar, area);
}

// ─── Transaction Detail View ─────────────────────────────

fn draw_tx_detail(f: &mut Frame, app: &App, tx: &Transaction) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Min(0),
            Constraint::Length(1),
        ])
        .split(f.area());

    draw_header(f, chunks[0]);

    let block = Block::default()
        .title(Line::from(vec![Span::styled(
            " Transaction Detail ",
            Style::default().fg(ACTIVE_BORDER).bold(),
        )]))
        .borders(Borders::ALL)
        .border_style(Style::default().fg(ACTIVE_BORDER));

    let inner = block.inner(chunks[1]);
    f.render_widget(block, chunks[1]);

    let status_text = if tx.success {
        "Success ✓"
    } else {
        "Failed ✗"
    };
    let status_color = if tx.success {
        SUCCESS_COLOR
    } else {
        ERROR_COLOR
    };

    let mut lines = vec![
        Line::from(""),
        Line::from(vec![
            Span::styled("  Signature:    ", Style::default().fg(DIM_COLOR)),
            Span::styled(&tx.signature, Style::default().fg(Color::White)),
        ]),
        Line::from(""),
        Line::from(vec![
            Span::styled("  Program:      ", Style::default().fg(DIM_COLOR)),
            Span::styled(&tx.program_id, Style::default().fg(HEADER_COLOR)),
        ]),
        Line::from(""),
        Line::from(vec![
            Span::styled("  Status:       ", Style::default().fg(DIM_COLOR)),
            Span::styled(status_text, Style::default().fg(status_color).bold()),
        ]),
        Line::from(""),
        Line::from(vec![
            Span::styled("  Block Slot:   ", Style::default().fg(DIM_COLOR)),
            Span::styled(
                format!("{}", tx.block_slot),
                Style::default().fg(Color::White),
            ),
        ]),
        Line::from(vec![
            Span::styled("  Timestamp:    ", Style::default().fg(DIM_COLOR)),
            Span::styled(
                tx.timestamp.format("%Y-%m-%d %H:%M:%S UTC").to_string(),
                Style::default().fg(Color::White),
            ),
        ]),
        Line::from(""),
        Line::from(vec![
            Span::styled("  Instruction:  ", Style::default().fg(DIM_COLOR)),
            Span::styled(
                tx.instruction_type.as_deref().unwrap_or("unknown"),
                Style::default().fg(Color::White),
            ),
        ]),
        Line::from(vec![
            Span::styled("  Compute Units:", Style::default().fg(DIM_COLOR)),
            Span::styled(
                format!(" {} ({})", tx.compute_units, format_cu(tx.compute_units)),
                Style::default().fg(Color::White),
            ),
        ]),
        Line::from(vec![
            Span::styled("  Fee:          ", Style::default().fg(DIM_COLOR)),
            Span::styled(
                format!(
                    "{} lamports ({:.6} SOL)",
                    tx.fee_lamports,
                    tx.fee_lamports as f64 / 1_000_000_000.0
                ),
                Style::default().fg(Color::White),
            ),
        ]),
    ];

    lines.extend(cu_profile_lines(tx));

    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        format!("  Accounts ({}):", tx.accounts.len()),
        Style::default().fg(DIM_COLOR),
    )));

    for (i, account) in tx.accounts.iter().enumerate() {
        lines.push(Line::from(vec![
            Span::styled(format!("    [{:>2}] ", i), Style::default().fg(DIM_COLOR)),
            Span::styled(account.as_str(), Style::default().fg(Color::White)),
        ]));
    }

    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        format!("  Parsed Instructions ({}):", tx.instructions.len()),
        Style::default().fg(DIM_COLOR),
    )));
    for (i, ix) in tx.instructions.iter().enumerate() {
        let kind = ix.display_name().unwrap_or("unknown");
        lines.push(Line::from(vec![
            Span::styled(format!("    [{:>2}] ", i), Style::default().fg(DIM_COLOR)),
            Span::styled(kind, Style::default().fg(HEADER_COLOR)),
            Span::styled("  ", Style::default().fg(DIM_COLOR)),
            Span::styled(ix.program_id.as_str(), Style::default().fg(Color::White)),
        ]));

        if !ix.accounts.is_empty() {
            lines.push(Line::from(vec![
                Span::styled("         accounts: ", Style::default().fg(DIM_COLOR)),
                Span::styled(ix.accounts.join(", "), Style::default().fg(Color::White)),
            ]));
        }
    }

    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        "  Classification:",
        Style::default().fg(DIM_COLOR),
    )));
    match &tx.classification {
        Some(classification) => {
            lines.push(Line::from(vec![
                Span::styled("    category: ", Style::default().fg(DIM_COLOR)),
                Span::styled(
                    classification.category.as_str(),
                    Style::default().fg(Color::White),
                ),
            ]));
            lines.push(Line::from(vec![
                Span::styled("    classifier: ", Style::default().fg(DIM_COLOR)),
                Span::styled(
                    classification.classifier.as_str(),
                    Style::default().fg(HEADER_COLOR),
                ),
            ]));
            lines.push(Line::from(vec![
                Span::styled("    confidence: ", Style::default().fg(DIM_COLOR)),
                Span::styled(
                    format!("{:.2}", classification.confidence),
                    Style::default().fg(Color::White),
                ),
            ]));
            lines.push(Line::from(vec![
                Span::styled("    summary: ", Style::default().fg(DIM_COLOR)),
                Span::styled(
                    classification.summary.as_str(),
                    Style::default().fg(Color::White),
                ),
            ]));
        }
        None => {
            lines.push(Line::from(vec![
                Span::styled("    headline type: ", Style::default().fg(DIM_COLOR)),
                Span::styled(
                    tx.instruction_type.as_deref().unwrap_or("unknown"),
                    Style::default().fg(Color::White),
                ),
            ]));
        }
    }

    if let Some(trace) = &tx.classification_debug {
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            format!(
                "  Classification Debug ({} decisions):",
                trace.decisions.len()
            ),
            Style::default().fg(DIM_COLOR),
        )));

        for decision in trace.decisions.iter().take(12) {
            let status = if decision.matched { "match" } else { "skip" };
            let status_color = if decision.matched {
                SUCCESS_COLOR
            } else {
                DIM_COLOR
            };

            lines.push(Line::from(vec![
                Span::styled("    ", Style::default().fg(DIM_COLOR)),
                Span::styled(status, Style::default().fg(status_color).bold()),
                Span::styled("  ", Style::default().fg(DIM_COLOR)),
                Span::styled(
                    decision.classifier.as_str(),
                    Style::default().fg(HEADER_COLOR),
                ),
                Span::styled("  ", Style::default().fg(DIM_COLOR)),
                Span::styled(decision.reason.as_str(), Style::default().fg(Color::White)),
            ]));
        }

        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            format!("  Derived Legs ({}):", trace.legs.len()),
            Style::default().fg(DIM_COLOR),
        )));

        for leg in trace.legs.iter().take(20) {
            lines.push(Line::from(vec![Span::styled(
                format!(
                    "    [ix {:>2}] {:<15} amt={} dec={:?} dir={:?}",
                    leg.instruction_index,
                    leg.instruction_kind,
                    leg.amount,
                    leg.decimals,
                    leg.direction
                ),
                Style::default().fg(Color::White),
            )]));
            lines.push(Line::from(vec![
                Span::styled("         ", Style::default().fg(DIM_COLOR)),
                Span::styled(
                    format!("{} -> {} ({})", leg.source, leg.destination, leg.asset_hint),
                    Style::default().fg(HEADER_COLOR),
                ),
            ]));
        }
    }

    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        "  Raw Transaction JSON:",
        Style::default().fg(DIM_COLOR),
    )));
    match serde_json::to_string_pretty(tx) {
        Ok(raw) => {
            for raw_line in raw.lines() {
                lines.push(Line::from(vec![
                    Span::styled("    ", Style::default().fg(DIM_COLOR)),
                    Span::styled(raw_line.to_string(), Style::default().fg(Color::White)),
                ]));
            }
        }
        Err(_) => {
            lines.push(Line::from(vec![
                Span::styled("    ", Style::default().fg(DIM_COLOR)),
                Span::styled(
                    "<failed to serialize transaction>",
                    Style::default().fg(ERROR_COLOR),
                ),
            ]));
        }
    }

    f.render_widget(
        Paragraph::new(lines)
            .wrap(Wrap { trim: false })
            .scroll((app.detail_scroll, 0)),
        inner,
    );

    // Status bar
    let bar = Paragraph::new(Line::from(vec![
        Span::styled(" Esc", Style::default().fg(HEADER_COLOR).bold()),
        Span::styled(" back  ", Style::default().fg(DIM_COLOR)),
        Span::styled("q", Style::default().fg(HEADER_COLOR).bold()),
        Span::styled(" quit", Style::default().fg(DIM_COLOR)),
    ]))
    .style(Style::default().bg(Color::DarkGray));
    f.render_widget(bar, chunks[2]);
}

// ─── Alert Detail View ───────────────────────────────────

fn draw_alert_detail(f: &mut Frame, app: &App, alert: &AlertEvent) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Min(0),
            Constraint::Length(1),
        ])
        .split(f.area());

    draw_header(f, chunks[0]);

    let block = Block::default()
        .title(Line::from(vec![Span::styled(
            " Alert Detail ",
            Style::default().fg(ALERT_COLOR).bold(),
        )]))
        .borders(Borders::ALL)
        .border_style(Style::default().fg(ALERT_COLOR));

    let inner = block.inner(chunks[1]);
    f.render_widget(block, chunks[1]);

    let lines = vec![
        Line::from(""),
        Line::from(vec![
            Span::styled("  Rule:      ", Style::default().fg(DIM_COLOR)),
            Span::styled(&alert.rule_name, Style::default().fg(ALERT_COLOR).bold()),
        ]),
        Line::from(vec![
            Span::styled("  Rule ID:   ", Style::default().fg(DIM_COLOR)),
            Span::styled(&alert.rule_id, Style::default().fg(Color::White)),
        ]),
        Line::from(""),
        Line::from(vec![
            Span::styled("  Program:   ", Style::default().fg(DIM_COLOR)),
            Span::styled(&alert.program_id, Style::default().fg(HEADER_COLOR)),
        ]),
        Line::from(""),
        Line::from(vec![
            Span::styled("  Metric:    ", Style::default().fg(DIM_COLOR)),
            Span::styled(&alert.metric, Style::default().fg(Color::White)),
        ]),
        Line::from(vec![
            Span::styled("  Value:     ", Style::default().fg(DIM_COLOR)),
            Span::styled(
                format!("{:.4}", alert.value),
                Style::default().fg(ERROR_COLOR),
            ),
        ]),
        Line::from(vec![
            Span::styled("  Threshold: ", Style::default().fg(DIM_COLOR)),
            Span::styled(
                format!("{}", alert.threshold),
                Style::default().fg(Color::White),
            ),
        ]),
        Line::from(""),
        Line::from(vec![
            Span::styled("  Fired At:  ", Style::default().fg(DIM_COLOR)),
            Span::styled(
                alert.fired_at.format("%Y-%m-%d %H:%M:%S UTC").to_string(),
                Style::default().fg(Color::White),
            ),
        ]),
    ];

    f.render_widget(Paragraph::new(lines).scroll((app.detail_scroll, 0)), inner);

    let bar = Paragraph::new(Line::from(vec![
        Span::styled(" Esc", Style::default().fg(HEADER_COLOR).bold()),
        Span::styled(" back  ", Style::default().fg(DIM_COLOR)),
        Span::styled("q", Style::default().fg(HEADER_COLOR).bold()),
        Span::styled(" quit", Style::default().fg(DIM_COLOR)),
    ]))
    .style(Style::default().bg(Color::DarkGray));
    f.render_widget(bar, chunks[2]);
}

fn format_cu(cu: u64) -> String {
    if cu >= 1_000_000 {
        format!("{:.1}M", cu as f64 / 1_000_000.0)
    } else if cu >= 1_000 {
        format!("{:.1}K", cu as f64 / 1_000.0)
    } else {
        format!("{}", cu)
    }
}

fn cu_profile_lines(tx: &Transaction) -> Vec<Line<'static>> {
    let profile = match &tx.cu_profile {
        Some(p) => p,
        None => {
            return vec![
                Line::from(""),
                Line::from(vec![
                    Span::styled("  CU Profile:  ", Style::default().fg(DIM_COLOR)),
                    Span::styled(
                        "unavailable (no log messages)",
                        Style::default().fg(DIM_COLOR),
                    ),
                ]),
            ];
        }
    };

    let top_level = profile.top_level_sorted_by_cu();
    let mut lines: Vec<Line<'static>> = Vec::new();
    lines.push(Line::from(""));

    let (badge_text, badge_color) = if profile.verified {
        ("verified ✓", SUCCESS_COLOR)
    } else {
        ("reconstruction incomplete ⚠", ALERT_COLOR)
    };

    lines.push(Line::from(vec![
        Span::styled("  CU Profile:  ", Style::default().fg(DIM_COLOR)),
        Span::styled(badge_text, Style::default().fg(badge_color)),
    ]));

    if top_level.is_empty() {
        lines.push(Line::from(Span::styled(
            "    (no top-level invocations parsed)",
            Style::default().fg(DIM_COLOR),
        )));
        return lines;
    }

    let max_cu = top_level
        .iter()
        .map(|inv| inv.consumed_cu.unwrap_or(NATIVE_PROGRAM_CU))
        .max()
        .unwrap_or(1)
        .max(1);

    const BAR_WIDTH: usize = 20;

    for (idx, inv) in top_level.iter().enumerate() {
        let cu = inv.consumed_cu.unwrap_or(NATIVE_PROGRAM_CU);
        let pct = if profile.reported_total > 0 {
            (cu as f64 / profile.reported_total as f64) * 100.0
        } else {
            0.0
        };

        let filled = ((cu as f64 / max_cu as f64) * BAR_WIDTH as f64).round() as usize;
        let filled = filled.clamp(1, BAR_WIDTH);
        let bar: String = "█".repeat(filled) + &" ".repeat(BAR_WIDTH - filled);

        let pid_short = if inv.program_id.len() > 20 {
            format!(
                "{}…{}",
                &inv.program_id[..10],
                &inv.program_id[inv.program_id.len() - 4..]
            )
        } else {
            inv.program_id.clone()
        };

        let is_native = inv.consumed_cu.is_none();
        let row_color = if inv.failed {
            ERROR_COLOR
        } else if is_native {
            DIM_COLOR
        } else {
            Color::White
        };

        let tags = match (is_native, inv.failed) {
            (true, true) => " (native, FAILED)".to_string(),
            (true, false) => " (native)".to_string(),
            (false, true) => " FAILED".to_string(),
            (false, false) => String::new(),
        };

        lines.push(Line::from(vec![
            Span::styled(format!("    [{:>2}] ", idx), Style::default().fg(DIM_COLOR)),
            Span::styled(
                format!("{:>10} CU  ", format_cu_full(cu)),
                Style::default().fg(row_color),
            ),
            Span::styled(bar, Style::default().fg(HEADER_COLOR)),
            Span::styled(format!("  {:>5.1}%  ", pct), Style::default().fg(DIM_COLOR)),
            Span::styled(
                format!("{}{}", pid_short, tags),
                Style::default().fg(row_color),
            ),
        ]));
    }

    lines.push(Line::from(vec![Span::styled(
        format!(
            "    total: {} CU reconstructed / {} CU reported",
            format_cu_full(profile.reconstructed_total),
            format_cu_full(profile.reported_total),
        ),
        Style::default().fg(DIM_COLOR),
    )]));

    if profile.native_overhead_cu > 0 {
        lines.push(Line::from(vec![Span::styled(
            format!(
                "    native program overhead: {} CU ({} × {} CU)",
                profile.native_overhead_cu,
                profile.native_overhead_cu / NATIVE_PROGRAM_CU,
                NATIVE_PROGRAM_CU,
            ),
            Style::default().fg(DIM_COLOR),
        )]));
    }

    lines
}

fn format_cu_full(cu: u64) -> String {
    let s = cu.to_string();
    let mut out = String::with_capacity(s.len() + s.len() / 3);
    for (i, ch) in s.chars().rev().enumerate() {
        if i > 0 && i % 3 == 0 {
            out.push(',');
        }
        out.push(ch);
    }
    out.chars().rev().collect()
}
