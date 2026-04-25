use std::io::{self, IsTerminal};
use std::path::{Path, PathBuf};
use std::time::Duration;

use crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};
use crossterm::execute;
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Alignment, Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Padding, Paragraph};
use ratatui::{Frame, Terminal};

const BANNER_GULF: [&str; 6] = [
    " ██████╗ ██╗   ██╗██╗     ███████╗",
    "██╔════╝ ██║   ██║██║     ██╔════╝",
    "██║  ███╗██║   ██║██║     █████╗  ",
    "██║   ██║██║   ██║██║     ██╔══╝  ",
    "╚██████╔╝╚██████╔╝███████╗██║     ",
    " ╚═════╝  ╚═════╝ ╚══════╝╚═╝     ",
];

const BANNER_WATCH: [&str; 6] = [
    "██╗    ██╗ █████╗ ████████╗ ██████╗██╗  ██╗",
    "██║    ██║██╔══██╗╚══██╔══╝██╔════╝██║  ██║",
    "██║ █╗ ██║███████║   ██║   ██║     ███████║",
    "██║███╗██║██╔══██║   ██║   ██║     ██╔══██║",
    "╚███╔███╔╝██║  ██║   ██║   ╚██████╗██║  ██║",
    " ╚══╝╚══╝ ╚═╝  ╚═╝   ╚═╝    ╚═════╝╚═╝  ╚═╝",
];

const BANNER_WIDTH: u16 = 43;

pub const GULF_COLOR: Color = Color::Rgb(0x9f, 0xd6, 0x7a);
pub const WATCH_COLOR: Color = Color::Rgb(0xe8, 0xb7, 0x5a);
const DIM: Color = Color::Rgb(0x88, 0x88, 0x88);

pub struct Setup {
    pub dir: PathBuf,
    pub ws_url: String,
    pub rpc_url: String,
}

pub fn run(cwd: PathBuf) -> io::Result<Option<Setup>> {
    if !io::stdin().is_terminal() || !io::stdout().is_terminal() {
        return Ok(None);
    }

    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let result = wizard_loop(&mut terminal, cwd);

    let _ = disable_raw_mode();
    let _ = execute!(terminal.backend_mut(), LeaveAlternateScreen);
    let _ = terminal.show_cursor();
    result
}

#[derive(Clone, Copy)]
enum Step {
    PickDir,
    EnterDirName,
    EnterWsUrl,
    EnterRpcUrl,
}

struct State {
    cwd: PathBuf,
    step: Step,
    dir_choice: usize,
    dir_input: String,
    ws_input: String,
    rpc_input: String,
    error: Option<String>,
    chosen_dir: Option<PathBuf>,
}

fn wizard_loop(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    cwd: PathBuf,
) -> io::Result<Option<Setup>> {
    let mut state = State {
        cwd,
        step: Step::PickDir,
        dir_choice: 0,
        dir_input: String::new(),
        ws_input: String::new(),
        rpc_input: String::new(),
        error: None,
        chosen_dir: None,
    };

    loop {
        terminal.draw(|f| draw(f, &state))?;

        if !event::poll(Duration::from_millis(250))? {
            continue;
        }
        let Event::Key(key) = event::read()? else {
            continue;
        };
        if key.kind != KeyEventKind::Press {
            continue;
        }

        if key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL) {
            return Ok(None);
        }

        match state.step {
            Step::PickDir => match key.code {
                KeyCode::Esc => return Ok(None),
                KeyCode::Up | KeyCode::Char('k') => {
                    if state.dir_choice > 0 {
                        state.dir_choice -= 1;
                    }
                }
                KeyCode::Down | KeyCode::Char('j') => {
                    if state.dir_choice < 2 {
                        state.dir_choice += 1;
                    }
                }
                KeyCode::Enter => {
                    state.error = None;
                    match state.dir_choice {
                        0 => {
                            state.chosen_dir = Some(state.cwd.clone());
                            state.step = Step::EnterWsUrl;
                        }
                        1 => state.step = Step::EnterDirName,
                        _ => return Ok(None),
                    }
                }
                _ => {}
            },
            Step::EnterDirName => match key.code {
                KeyCode::Esc => {
                    state.dir_input.clear();
                    state.error = None;
                    state.step = Step::PickDir;
                }
                KeyCode::Backspace => {
                    state.dir_input.pop();
                }
                KeyCode::Enter => {
                    let name = state.dir_input.trim().to_string();
                    if name.is_empty() {
                        state.error = Some("Directory name cannot be empty.".into());
                        continue;
                    }
                    let target = state.cwd.join(&name);
                    if let Err(e) = std::fs::create_dir_all(&target) {
                        state.error =
                            Some(format!("Failed to create {}: {}", target.display(), e));
                        continue;
                    }
                    if has_existing_config(&target) {
                        state.error = Some(format!(
                            "{} already has a gulfwatch.toml. Pick another name.",
                            target.display()
                        ));
                        continue;
                    }
                    state.chosen_dir = Some(target);
                    state.error = None;
                    state.step = Step::EnterWsUrl;
                }
                KeyCode::Char(c) => state.dir_input.push(c),
                _ => {}
            },
            Step::EnterWsUrl => match key.code {
                KeyCode::Esc => {
                    state.ws_input.clear();
                    state.error = None;
                    state.step = Step::PickDir;
                }
                KeyCode::Backspace => {
                    state.ws_input.pop();
                }
                KeyCode::Enter => {
                    let v = state.ws_input.trim().to_string();
                    if v.is_empty() {
                        state.error = Some("WebSocket URL cannot be empty.".into());
                        continue;
                    }
                    state.error = None;
                    state.step = Step::EnterRpcUrl;
                }
                KeyCode::Char(c) => state.ws_input.push(c),
                _ => {}
            },
            Step::EnterRpcUrl => match key.code {
                KeyCode::Esc => {
                    state.rpc_input.clear();
                    state.error = None;
                    state.step = Step::EnterWsUrl;
                }
                KeyCode::Backspace => {
                    state.rpc_input.pop();
                }
                KeyCode::Enter => {
                    let rpc = state.rpc_input.trim().to_string();
                    if rpc.is_empty() {
                        state.error = Some("RPC URL cannot be empty.".into());
                        continue;
                    }
                    return Ok(Some(Setup {
                        dir: state.chosen_dir.take().expect("dir chosen by this step"),
                        ws_url: state.ws_input.trim().to_string(),
                        rpc_url: rpc,
                    }));
                }
                KeyCode::Char(c) => state.rpc_input.push(c),
                _ => {}
            },
        }
    }
}

fn has_existing_config(dir: &Path) -> bool {
    dir.join("gulfwatch.toml").is_file()
}

fn draw(f: &mut Frame, state: &State) {
    let area = f.area();

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(15),
            Constraint::Min(7),
            Constraint::Length(2),
        ])
        .split(area);

    draw_banner(f, chunks[0]);
    draw_step(f, chunks[1], state);
    draw_footer(f, chunks[2], state.step);
}

fn draw_banner(f: &mut Frame, area: Rect) {
    let mut lines: Vec<Line> = Vec::with_capacity(15);
    for s in BANNER_GULF.iter() {
        lines.push(Line::from(Span::styled(
            *s,
            Style::default().fg(GULF_COLOR),
        )));
    }
    for s in BANNER_WATCH.iter() {
        lines.push(Line::from(Span::styled(
            *s,
            Style::default().fg(WATCH_COLOR),
        )));
    }
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        "Runtime intelligence for Solana",
        Style::default().fg(DIM).add_modifier(Modifier::ITALIC),
    )));

    let centered = center_horizontal(area, BANNER_WIDTH);
    let p = Paragraph::new(lines).alignment(Alignment::Left);
    f.render_widget(p, centered);
}

fn center_horizontal(area: Rect, width: u16) -> Rect {
    if area.width <= width {
        return area;
    }
    let pad = (area.width - width) / 2;
    Rect {
        x: area.x + pad,
        y: area.y,
        width,
        height: area.height,
    }
}

fn draw_step(f: &mut Frame, area: Rect, state: &State) {
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(DIM))
        .padding(Padding::uniform(1));
    let inner = block.inner(area);
    f.render_widget(block, area);

    match state.step {
        Step::PickDir => render_pick_dir(f, inner, state),
        Step::EnterDirName => render_input(
            f,
            inner,
            "New directory name",
            &state.dir_input,
            "Created as a subdirectory of the current path.",
            state.error.as_deref(),
        ),
        Step::EnterWsUrl => render_input(
            f,
            inner,
            "Solana WebSocket URL",
            &state.ws_input,
            "From Helius, Quicknode, Triton, or any Solana RPC provider. Starts with wss://",
            state.error.as_deref(),
        ),
        Step::EnterRpcUrl => render_input(
            f,
            inner,
            "Solana RPC URL",
            &state.rpc_input,
            "Usually the HTTPS endpoint paired with your WebSocket URL.",
            state.error.as_deref(),
        ),
    }
}

fn render_pick_dir(f: &mut Frame, area: Rect, state: &State) {
    let here = state.cwd.display().to_string();
    let options = [
        format!("Use this directory  ({})", here),
        "Create a new subdirectory".to_string(),
        "Quit".to_string(),
    ];
    let mut lines = vec![
        Line::from(Span::styled(
            "Where should we set up GulfWatch?",
            Style::default()
                .fg(WATCH_COLOR)
                .add_modifier(Modifier::BOLD),
        )),
        Line::from(""),
    ];
    for (i, opt) in options.iter().enumerate() {
        let selected = i == state.dir_choice;
        let style = if selected {
            Style::default()
                .fg(GULF_COLOR)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(Color::Reset)
        };
        let marker = if selected { "▶ " } else { "  " };
        lines.push(Line::from(vec![
            Span::styled(marker, style),
            Span::styled(opt.clone(), style),
        ]));
    }
    if let Some(err) = &state.error {
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            err.clone(),
            Style::default().fg(Color::LightRed),
        )));
    }
    f.render_widget(Paragraph::new(lines), area);
}

fn render_input(
    f: &mut Frame,
    area: Rect,
    prompt: &str,
    value: &str,
    hint: &str,
    error: Option<&str>,
) {
    let mut lines = vec![
        Line::from(Span::styled(
            prompt,
            Style::default()
                .fg(WATCH_COLOR)
                .add_modifier(Modifier::BOLD),
        )),
        Line::from(Span::styled(hint, Style::default().fg(DIM))),
        Line::from(""),
        Line::from(vec![
            Span::styled("> ", Style::default().fg(GULF_COLOR)),
            Span::styled(value.to_string(), Style::default().fg(Color::Reset)),
            Span::styled(
                "█",
                Style::default()
                    .fg(GULF_COLOR)
                    .add_modifier(Modifier::SLOW_BLINK),
            ),
        ]),
    ];
    if let Some(err) = error {
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            err,
            Style::default().fg(Color::LightRed),
        )));
    }
    f.render_widget(Paragraph::new(lines), area);
}

fn draw_footer(f: &mut Frame, area: Rect, step: Step) {
    let hint = match step {
        Step::PickDir => "↑↓ navigate    Enter select    Esc / Ctrl+C quit",
        _ => "Enter confirm    Esc back    Ctrl+C quit",
    };
    let p = Paragraph::new(Span::styled(hint, Style::default().fg(DIM)))
        .alignment(Alignment::Center);
    f.render_widget(p, area);
}
