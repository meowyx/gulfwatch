use gulfwatch_core::alert::AlertEvent;
use gulfwatch_core::transaction::Transaction;
use ratatui::{
    prelude::*,
    widgets::{Block, Borders, Cell, Paragraph, Row, Scrollbar, ScrollbarOrientation,
              ScrollbarState, Table, Wrap},
};

use crate::app::{App, View};

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
        View::TransactionDetail(tx) => draw_tx_detail(f, tx),
        View::AlertDetail(alert) => draw_alert_detail(f, alert),
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
        Span::styled("Solana Program Observability", Style::default().fg(DIM_COLOR)),
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
        .constraints([Constraint::Percentage(55), Constraint::Percentage(45)])
        .split(area);

    let right_chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Percentage(60), Constraint::Percentage(40)])
        .split(h_chunks[1]);

    draw_transactions(f, h_chunks[0], app);
    draw_metrics(f, right_chunks[0], app);
    draw_alerts(f, right_chunks[1], app);
}

fn panel_border<'a>(title: &'a str, panel_idx: usize, app: &'a App) -> Block<'a> {
    let is_active = app.active_panel == panel_idx;
    let border_color = if is_active { ACTIVE_BORDER } else { INACTIVE_BORDER };

    Block::default()
        .title(Line::from(vec![Span::styled(
            format!(" {} ", title),
            Style::default().fg(border_color).bold(),
        )]))
        .borders(Borders::ALL)
        .border_style(Style::default().fg(border_color))
}

fn draw_transactions(f: &mut Frame, area: Rect, app: &App) {
    let block = panel_border("Transactions [1]", 0, app);
    let inner = block.inner(area);
    f.render_widget(block, area);

    if app.transactions.is_empty() {
        let msg = Paragraph::new("Waiting for transactions...")
            .style(Style::default().fg(DIM_COLOR))
            .alignment(Alignment::Center);
        f.render_widget(msg, inner);
        return;
    }

    let header = Row::new(vec![
        Cell::from("Sig").style(Style::default().fg(HEADER_COLOR)),
        Cell::from("").style(Style::default().fg(HEADER_COLOR)),
        Cell::from("Type").style(Style::default().fg(HEADER_COLOR)),
        Cell::from("CU").style(Style::default().fg(HEADER_COLOR)),
        Cell::from("Fee").style(Style::default().fg(HEADER_COLOR)),
        Cell::from("Time").style(Style::default().fg(HEADER_COLOR)),
    ])
    .height(1);

    let is_active = app.active_panel == 0;
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

    let rows: Vec<Row> = app
        .transactions
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

            let mut row = Row::new(vec![
                Cell::from(sig_short),
                Cell::from(if tx.success { "✓" } else { "✗" }).style(status_style),
                Cell::from(
                    tx.instruction_type.as_deref().unwrap_or("—").to_string(),
                ),
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
        Constraint::Length(10),
        Constraint::Length(8),
        Constraint::Length(10),
        Constraint::Length(8),
    ];

    let table = Table::new(rows, widths).header(header);
    f.render_widget(table, inner);

    // Scrollbar
    if is_active && app.transactions.len() > visible_rows {
        let mut scrollbar_state = ScrollbarState::new(app.transactions.len())
            .position(app.selected);
        f.render_stateful_widget(
            Scrollbar::new(ScrollbarOrientation::VerticalRight),
            area,
            &mut scrollbar_state,
        );
    }
}

fn draw_metrics(f: &mut Frame, area: Rect, app: &App) {
    let block = panel_border("Metrics [2]", 1, app);
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

    let programs = match app.state.monitored_programs.try_read() {
        Ok(p) => p,
        Err(_) => return,
    };

    if programs.is_empty() {
        f.render_widget(
            Paragraph::new("No programs monitored")
                .style(Style::default().fg(DIM_COLOR))
                .alignment(Alignment::Center),
            inner,
        );
        return;
    }

    let mut lines: Vec<Line> = Vec::new();

    for pid in programs.iter() {
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
                Span::styled(format!("{}", summary.tx_count), Style::default().fg(Color::White)),
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
                Span::styled(format!("{:.1}%", summary.error_rate * 100.0), Style::default().fg(err_color)),
                Span::styled(format!(" ({} errors)", summary.error_count), Style::default().fg(DIM_COLOR)),
            ]));

            lines.push(Line::from(vec![
                Span::styled("  Avg CU:       ", Style::default().fg(DIM_COLOR)),
                Span::styled(format_cu(summary.avg_compute_units as u64), Style::default().fg(Color::White)),
            ]));

            if !summary.top_instructions.is_empty() {
                lines.push(Line::from(Span::styled(
                    "  Top Instrs:",
                    Style::default().fg(DIM_COLOR),
                )));
                for instr in summary.top_instructions.iter().take(5) {
                    lines.push(Line::from(vec![
                        Span::styled(format!("    {} ", instr.instruction_type), Style::default().fg(Color::White)),
                        Span::styled(format!("({})", instr.count), Style::default().fg(DIM_COLOR)),
                    ]));
                }
            }

            lines.push(Line::from(""));
        }
    }

    f.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), inner);
}

fn draw_alerts(f: &mut Frame, area: Rect, app: &App) {
    let count = app.alerts.len();
    let title = if count > 0 {
        format!("Alerts [3] ({})", count)
    } else {
        "Alerts [3]".to_string()
    };

    let block = panel_border(&title, 2, app);
    let inner = block.inner(area);
    f.render_widget(block, area);

    if app.alerts.is_empty() {
        f.render_widget(
            Paragraph::new("No alerts")
                .style(Style::default().fg(DIM_COLOR))
                .alignment(Alignment::Center),
            inner,
        );
        return;
    }

    let is_active = app.active_panel == 2;
    let visible_rows = inner.height as usize;
    let scroll = if is_active && app.selected >= visible_rows {
        app.selected - visible_rows + 1
    } else {
        0
    };

    let lines: Vec<Line> = app
        .alerts
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
                    " {} ⚠ {}  ({}: {:.2} > {})",
                    time, alert.rule_name, alert.metric, alert.value, alert.threshold
                ),
                style,
            )
        })
        .collect();

    f.render_widget(Paragraph::new(lines), inner);
}

fn draw_status_bar(f: &mut Frame, area: Rect, app: &App) {
    let panel_names = ["Transactions", "Metrics", "Alerts"];
    let active = panel_names[app.active_panel];

    let bar = Paragraph::new(Line::from(vec![
        Span::styled(" q", Style::default().fg(HEADER_COLOR).bold()),
        Span::styled(" quit  ", Style::default().fg(DIM_COLOR)),
        Span::styled("Tab", Style::default().fg(HEADER_COLOR).bold()),
        Span::styled(" switch  ", Style::default().fg(DIM_COLOR)),
        Span::styled("↑↓", Style::default().fg(HEADER_COLOR).bold()),
        Span::styled(" scroll  ", Style::default().fg(DIM_COLOR)),
        Span::styled("Enter", Style::default().fg(HEADER_COLOR).bold()),
        Span::styled(" detail  ", Style::default().fg(DIM_COLOR)),
        Span::styled("Esc", Style::default().fg(HEADER_COLOR).bold()),
        Span::styled(" back  ", Style::default().fg(DIM_COLOR)),
        Span::styled("│ ", Style::default().fg(DIM_COLOR)),
        Span::styled(format!("{} ", active), Style::default().fg(Color::White)),
        Span::styled("│ ", Style::default().fg(DIM_COLOR)),
        Span::styled(format!("{} txs ", app.transactions.len()), Style::default().fg(Color::White)),
        Span::styled(
            format!("{} alerts", app.alerts.len()),
            Style::default().fg(if app.alerts.is_empty() { DIM_COLOR } else { ALERT_COLOR }),
        ),
    ]))
    .style(Style::default().bg(Color::DarkGray));

    f.render_widget(bar, area);
}

// ─── Transaction Detail View ─────────────────────────────

fn draw_tx_detail(f: &mut Frame, tx: &Transaction) {
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
        .title(Line::from(vec![
            Span::styled(" Transaction Detail ", Style::default().fg(ACTIVE_BORDER).bold()),
        ]))
        .borders(Borders::ALL)
        .border_style(Style::default().fg(ACTIVE_BORDER));

    let inner = block.inner(chunks[1]);
    f.render_widget(block, chunks[1]);

    let status_text = if tx.success { "Success ✓" } else { "Failed ✗" };
    let status_color = if tx.success { SUCCESS_COLOR } else { ERROR_COLOR };

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
            Span::styled(format!("{}", tx.block_slot), Style::default().fg(Color::White)),
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
                format!("{} lamports ({:.6} SOL)", tx.fee_lamports, tx.fee_lamports as f64 / 1_000_000_000.0),
                Style::default().fg(Color::White),
            ),
        ]),
        Line::from(""),
        Line::from(Span::styled(
            format!("  Accounts ({}):", tx.accounts.len()),
            Style::default().fg(DIM_COLOR),
        )),
    ];

    for (i, account) in tx.accounts.iter().enumerate() {
        lines.push(Line::from(vec![
            Span::styled(format!("    [{:>2}] ", i), Style::default().fg(DIM_COLOR)),
            Span::styled(account.as_str(), Style::default().fg(Color::White)),
        ]));
    }

    f.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), inner);

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

fn draw_alert_detail(f: &mut Frame, alert: &AlertEvent) {
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
        .title(Line::from(vec![
            Span::styled(" Alert Detail ", Style::default().fg(ALERT_COLOR).bold()),
        ]))
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
            Span::styled(format!("{:.4}", alert.value), Style::default().fg(ERROR_COLOR)),
        ]),
        Line::from(vec![
            Span::styled("  Threshold: ", Style::default().fg(DIM_COLOR)),
            Span::styled(format!("{}", alert.threshold), Style::default().fg(Color::White)),
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

    f.render_widget(Paragraph::new(lines), inner);

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
