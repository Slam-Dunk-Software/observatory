use std::{collections::HashMap, io, path::PathBuf, time::Duration};

use anyhow::Result;
use crossterm::{
    event::{self, Event, KeyCode, KeyEventKind},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{
    backend::CrosstermBackend,
    layout::Alignment,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph, Wrap},
    Terminal,
};
use rusqlite::Connection;

use crate::{db, nodes};

pub fn run() -> Result<()> {
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let db_path = dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("/tmp"))
        .join(".epm/services/observatory.db");
    let conn = Connection::open(&db_path)?;

    let result = event_loop(&mut terminal, &conn);

    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    terminal.show_cursor()?;

    result
}

fn event_loop(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    conn: &Connection,
) -> Result<()> {
    loop {
        terminal.draw(|f| draw(f, conn))?;

        if event::poll(Duration::from_millis(250))? {
            if let Event::Key(key) = event::read()? {
                if key.kind == KeyEventKind::Press {
                    match key.code {
                        KeyCode::Char('q') | KeyCode::Char('Q') | KeyCode::Esc => return Ok(()),
                        _ => {}
                    }
                }
            }
        }
    }
}

fn draw(f: &mut ratatui::Frame, conn: &Connection) {
    let area = f.area();

    let node_list = nodes::load_nodes();
    let node_states = db::all_node_states(conn).unwrap_or_default();
    let node_svc_map = db::latest_node_service_statuses(conn).unwrap_or_default();
    let service_states = db::all_states(conn).unwrap_or_default();

    let state_map: HashMap<&str, &db::NodeSnapshot> =
        node_states.iter().map(|s| (s.node.as_str(), s)).collect();

    let mut lines: Vec<Line> = vec![];

    // ── Title ─────────────────────────────────────────────────────────────────
    lines.push(Line::from(vec![
        Span::styled(
            "◆ Observatory",
            Style::default()
                .fg(Color::Rgb(160, 160, 200))
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            "   [q] quit",
            Style::default().fg(Color::Rgb(70, 70, 100)),
        ),
    ]));
    lines.push(Line::raw(""));

    // ── Nodes ─────────────────────────────────────────────────────────────────
    if !node_list.is_empty() {
        lines.push(dim_line("── nodes ──────────────────────────────────────────────"));

        for node in &node_list {
            lines.push(Line::raw(""));

            let (pip_color, status_str) = match state_map.get(node.name.as_str()) {
                Some(s) => match s.last_status.as_str() {
                    "ok"          => (Color::Rgb(76, 175, 80),  "ok"),
                    "warn"        => (Color::Rgb(255, 152, 0),  "warn"),
                    "alert"       => (Color::Rgb(244, 67, 54),  "alert"),
                    "unreachable" => (Color::Rgb(100, 100, 130), "unreachable"),
                    _             => (Color::Rgb(100, 100, 130), "unknown"),
                },
                None => (Color::Rgb(100, 100, 130), "no data"),
            };

            // Node name + status
            lines.push(Line::from(vec![
                Span::styled("  ● ", Style::default().fg(pip_color)),
                Span::styled(
                    node.name.clone(),
                    Style::default()
                        .fg(Color::Rgb(200, 200, 240))
                        .add_modifier(Modifier::BOLD),
                ),
                Span::styled(
                    format!("  {}", status_str),
                    Style::default().fg(pip_color),
                ),
            ]));

            if let Some(state) = state_map.get(node.name.as_str()) {
                // Disk
                if let Some(d) = state.disk_pct {
                    let warn = node.disk_alert_threshold.saturating_sub(10);
                    let bar_color = if d >= node.disk_alert_threshold {
                        Color::Rgb(244, 67, 54)
                    } else if d >= warn {
                        Color::Rgb(255, 152, 0)
                    } else {
                        Color::Rgb(76, 175, 80)
                    };
                    lines.push(Line::from(vec![
                        Span::styled("    disk  ", Style::default().fg(Color::Rgb(70, 70, 110))),
                        Span::styled(bar(d, 20), Style::default().fg(bar_color)),
                        Span::styled(
                            format!("  {}%", d),
                            Style::default().fg(Color::Rgb(150, 150, 190)),
                        ),
                    ]));
                }

                // CPU + Mem on one line
                let mut metric_spans = vec![Span::styled(
                    "    ",
                    Style::default(),
                )];
                if let Some(l) = state.cpu_load {
                    let color = if l >= node.cpu_alert_threshold {
                        Color::Rgb(244, 67, 54)
                    } else if l >= 2.0 {
                        Color::Rgb(255, 152, 0)
                    } else {
                        Color::Rgb(150, 150, 190)
                    };
                    metric_spans.push(Span::styled(
                        "cpu  ",
                        Style::default().fg(Color::Rgb(70, 70, 110)),
                    ));
                    metric_spans.push(Span::styled(
                        format!("{:.2}    ", l),
                        Style::default().fg(color),
                    ));
                }
                if let Some(m) = state.mem_pct {
                    let bar_color = if m >= 90 {
                        Color::Rgb(244, 67, 54)
                    } else if m >= 75 {
                        Color::Rgb(255, 152, 0)
                    } else {
                        Color::Rgb(100, 149, 237)
                    };
                    metric_spans.push(Span::styled(
                        "mem  ",
                        Style::default().fg(Color::Rgb(70, 70, 110)),
                    ));
                    metric_spans.push(Span::styled(
                        bar(m, 20),
                        Style::default().fg(bar_color),
                    ));
                    metric_spans.push(Span::styled(
                        format!("  {}%", m),
                        Style::default().fg(Color::Rgb(150, 150, 190)),
                    ));
                }
                if state.cpu_load.is_some() || state.mem_pct.is_some() {
                    lines.push(Line::from(metric_spans));
                }
            }

            // Services
            if !node.services.is_empty() {
                let empty_map = HashMap::new();
                let svc_map = node_svc_map.get(&node.name).unwrap_or(&empty_map);
                let mut svc_spans = vec![Span::styled("    ", Style::default())];
                for svc in &node.services {
                    let active = svc_map.get(svc).copied().unwrap_or(false);
                    let (dot_color, svc_color) = if active {
                        (Color::Rgb(76, 175, 80), Color::Rgb(140, 190, 140))
                    } else {
                        (Color::Rgb(244, 67, 54), Color::Rgb(190, 100, 100))
                    };
                    svc_spans.push(Span::styled(
                        svc.clone(),
                        Style::default().fg(svc_color),
                    ));
                    svc_spans.push(Span::styled(
                        " ●  ",
                        Style::default().fg(dot_color),
                    ));
                }
                lines.push(Line::from(svc_spans));
            }
        }

        lines.push(Line::raw(""));
    }

    // ── Services ──────────────────────────────────────────────────────────────
    if !service_states.is_empty() {
        lines.push(dim_line("── services ────────────────────────────────────────────"));
        lines.push(Line::raw(""));

        for s in &service_states {
            let (pip_color, status_color) = match s.last_status.as_str() {
                "running"  => (Color::Rgb(76, 175, 80),  Color::Rgb(76, 175, 80)),
                "degraded" => (Color::Rgb(255, 152, 0),  Color::Rgb(255, 152, 0)),
                _          => (Color::Rgb(68, 68, 102),  Color::Rgb(80, 80, 110)),
            };
            lines.push(Line::from(vec![
                Span::styled("  ", Style::default()),
                Span::styled(
                    format!("{:<22}", s.service),
                    Style::default().fg(Color::Rgb(190, 190, 230)),
                ),
                Span::styled("● ", Style::default().fg(pip_color)),
                Span::styled(
                    s.last_status.clone(),
                    Style::default().fg(status_color),
                ),
            ]));
        }

        lines.push(Line::raw(""));
    }

    if service_states.is_empty() && node_list.is_empty() {
        lines.push(Line::from(Span::styled(
            "  no data — is observatory server running?",
            Style::default().fg(Color::Rgb(80, 80, 110)),
        )));
    }

    // ── Footer ────────────────────────────────────────────────────────────────
    let now = chrono::Local::now().format("%H:%M:%S").to_string();
    lines.push(dim_line(&format!(
        "── last rendered: {} ─────────────────────────────────────",
        now
    )));

    let paragraph = Paragraph::new(lines)
        .block(Block::default().borders(Borders::NONE))
        .alignment(Alignment::Left)
        .wrap(Wrap { trim: false });

    f.render_widget(paragraph, area);
}

fn bar(pct: u8, width: usize) -> String {
    let filled = ((pct as usize) * width / 100).min(width);
    let empty = width - filled;
    format!("{}{}", "█".repeat(filled), "░".repeat(empty))
}

fn dim_line(s: &str) -> Line<'static> {
    Line::from(Span::styled(
        s.to_string(),
        Style::default().fg(Color::Rgb(50, 50, 80)),
    ))
}
