use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use axum::{extract::State, response::Html};
use rusqlite::Connection;
use serde::Deserialize;

use crate::{db, nodes};

// ── services.toml reader (for port → URL resolution) ─────────────────────────

#[derive(Deserialize, Default)]
struct ServicesFile {
    #[serde(default)]
    services: HashMap<String, ServiceEntry>,
}

#[derive(Deserialize)]
struct ServiceEntry {
    port: u16,
}

fn load_service_ports() -> HashMap<String, u16> {
    let path = dirs::home_dir()
        .unwrap_or_else(|| std::path::PathBuf::from("/tmp"))
        .join(".epc/services.toml");
    let raw = std::fs::read_to_string(&path).unwrap_or_default();
    let sf: ServicesFile = toml::from_str(&raw).unwrap_or_default();
    sf.services.into_iter().map(|(k, v)| (k, v.port)).collect()
}

pub async fn handler(State(db): State<Arc<Mutex<Connection>>>) -> Html<String> {
    Html(render(&db))
}

pub fn render_log_page(service: &str, content: &str) -> String {
    let escaped = html_escape_log(content);
    format!(r#"<!DOCTYPE html>
<html lang="en">
<head>
  <meta charset="UTF-8">
  <meta name="viewport" content="width=device-width, initial-scale=1">
  <title>{service} — logs</title>
  <style>
    * {{ box-sizing: border-box; margin: 0; padding: 0; }}
    body {{
      background: #0f0f1a;
      color: #c8c8e8;
      font-family: 'SF Mono', 'Menlo', monospace;
      font-size: 12px;
      padding: 16px;
    }}
    .header {{
      display: flex;
      align-items: center;
      gap: 12px;
      margin-bottom: 16px;
    }}
    a.back {{
      color: #6060a0;
      text-decoration: none;
      font-size: 12px;
    }}
    a.back:hover {{ color: #a0a0c0; }}
    h1 {{
      font-size: 14px;
      font-weight: 600;
      color: #a0a0c0;
      letter-spacing: 0.05em;
    }}
    pre {{
      white-space: pre;
      overflow-x: auto;
      line-height: 1.6;
      color: #c8c8e8;
    }}
  </style>
</head>
<body>
  <div class="header">
    <a class="back" href="/">← Observatory</a>
    <h1>{service}</h1>
  </div>
  <pre>{escaped}</pre>
</body>
</html>"#)
}

fn strip_ansi(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\x1b' {
            // consume ESC [ ... <final byte in 0x40–0x7E>
            if chars.peek() == Some(&'[') {
                chars.next();
                for ch in chars.by_ref() {
                    if ('\x40'..='\x7e').contains(&ch) { break; }
                }
            }
        } else {
            out.push(c);
        }
    }
    out
}

fn html_escape_log(s: &str) -> String {
    let stripped = strip_ansi(s);
    stripped.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
}

fn status_pip(status: &str) -> String {
    format!("<span class=\"pip {status}\"></span>")
}

fn dot(status: &str) -> &'static str {
    // \u{FE0E} is the Unicode text variation selector — forces text rendering
    // instead of emoji on iOS, which would otherwise replace ● with a color emoji.
    match status {
        "running"  => "<span class=\"dot running\">\u{25CF}\u{FE0E}</span>",
        "degraded" => "<span class=\"dot degraded\">\u{25D0}\u{FE0E}</span>",
        _          => "<span class=\"dot stopped\">\u{25CB}\u{FE0E}</span>",
    }
}

fn progress_bar_html(pct: u8, warn_threshold: u8, alert_threshold: u8) -> String {
    let filled = (pct as usize * 20 / 100).min(20);
    let empty = 20 - filled;
    let color = if pct >= alert_threshold {
        "#f44336"
    } else if pct >= warn_threshold {
        "#ff9800"
    } else {
        "#4caf50"
    };
    let bar: String = "█".repeat(filled) + &"░".repeat(empty);
    format!(r#"<span style="color:{color}">{bar}</span> <span style="color:#9090b0">{pct}%</span>"#)
}

fn render(db: &Arc<Mutex<Connection>>) -> String {
    let conn = db.lock().unwrap();

    let node_list = nodes::load_nodes();
    let node_states = db::all_node_states(&conn).unwrap_or_default();
    let node_svc_map = db::latest_node_service_statuses(&conn).unwrap_or_default();

    let states = db::all_states(&conn).unwrap_or_default();
    let ports = load_service_ports();
    let host = std::env::var("HOST").unwrap_or_else(|_| "localhost".to_string());

    // ── Node cards ────────────────────────────────────────────────────────────
    let mut node_section = String::new();
    if !node_list.is_empty() {
        let state_map: std::collections::HashMap<&str, &db::NodeSnapshot> =
            node_states.iter().map(|s| (s.node.as_str(), s)).collect();

        let mut node_cards = String::new();
        for node in &node_list {
            let (pip_class, status_label) = match state_map.get(node.name.as_str()) {
                Some(s) => match s.last_status.as_str() {
                    "ok"          => ("running",  "ok"),
                    "warn"        => ("degraded", "warn"),
                    "alert"       => ("degraded", "alert"),
                    "unreachable" => ("stopped",  "unreachable"),
                    _             => ("stopped",  "unknown"),
                },
                None => ("stopped", "no data"),
            };

            let disk_html = state_map
                .get(node.name.as_str())
                .and_then(|s| s.disk_pct)
                .map(|d| {
                    let warn = node.disk_alert_threshold.saturating_sub(10);
                    format!(
                        r#"<div class="node-metric"><span class="metric-label">disk</span>{}</div>"#,
                        progress_bar_html(d, warn, node.disk_alert_threshold)
                    )
                })
                .unwrap_or_default();

            let cpu_html = state_map
                .get(node.name.as_str())
                .and_then(|s| s.cpu_load)
                .map(|l| {
                    let color = if l >= node.cpu_alert_threshold { "#f44336" }
                        else if l >= node.cpu_alert_threshold * 0.8 as f64 { "#ff9800" }
                        else { "#9090b0" };
                    format!(
                        r#"<div class="node-metric"><span class="metric-label">cpu</span><span style="color:{color}">{:.2}</span></div>"#,
                        l
                    )
                })
                .unwrap_or_default();

            let mem_html = state_map
                .get(node.name.as_str())
                .and_then(|s| s.mem_pct)
                .map(|m| {
                    format!(
                        r#"<div class="node-metric"><span class="metric-label">mem</span>{}</div>"#,
                        progress_bar_html(m, 75, 90)
                    )
                })
                .unwrap_or_default();

            let svc_html = if !node.services.is_empty() {
                let empty_map = std::collections::HashMap::new();
                let svc_map = node_svc_map.get(&node.name).unwrap_or(&empty_map);
                let badges: String = node
                    .services
                    .iter()
                    .map(|svc| {
                        let active = svc_map.get(svc).copied().unwrap_or(false);
                        let color = if active { "#4caf50" } else { "#f44336" };
                        let node_name = &node.name;
                        format!(
                            r#"<a href="/logs/node/{node_name}/{svc}" class="svc-badge"><span style="color:{color}">●</span> {svc}</a>"#
                        )
                    })
                    .collect();
                format!(r#"<div class="node-services">{badges}</div>"#)
            } else {
                String::new()
            };

            node_cards.push_str(&format!(
                r#"<div class="card">
  <div class="card-header">
    <span class="svc-name">{name}</span>
    <span class="badge"><span class="pip {pip_class}"></span>{status_label}</span>
  </div>
  {disk_html}{cpu_html}{mem_html}{svc_html}
</div>
"#,
                name = node.name,
            ));
        }

        node_section = format!(
            r#"<div class="section-label">nodes</div>{node_cards}<div class="section-label">services</div>"#
        );
    }

    let mut cards = String::new();
    for s in &states {
        let checks = db::recent_checks(&conn, &s.service, 40).unwrap_or_default();

        let latest_ms = checks.first().and_then(|c| c.response_ms);
        let ms_label = match latest_ms {
            Some(ms) => format!("{ms}ms"),
            None => "--".to_string(),
        };

        // Dots: checks are newest-first, we render oldest→newest (left→right)
        let mut dots = String::new();
        for check in checks.iter().rev() {
            dots.push_str(dot(&check.status));
        }

        let pip = status_pip(&s.last_status);
        let name = &s.service;
        let last_checked = &s.last_checked;
        let ci_badge = match &s.repo_url {
            Some(url) => format!(
                r#"<a href="{url}/actions" target="_blank" class="ci-link"><img src="{url}/actions/workflows/ci.yml/badge.svg" alt="CI" class="ci-badge" loading="lazy" onerror="this.closest('.ci-link').remove()"></a>"#
            ),
            None => String::new(),
        };

        let svc_name_html = match ports.get(name) {
            Some(port) => {
                let url = format!("http://{}:{}", host, port);
                format!(r#"<a href="{url}" target="_blank" class="svc-link">{name}</a>"#)
            }
            None => name.to_string(),
        };

        cards.push_str(&format!(
            r#"<div class="card">
  <div class="card-header">
    <span class="svc-name">{svc_name_html}</span>
    <span class="badge">{pip}{status}</span>
  </div>
  <div class="card-meta">{ms_label} &nbsp;·&nbsp; {last_checked}</div>
  <div class="sparkline">{dots}</div>
  <div class="card-actions">
    {ci_badge}
    <a href="/logs/{name}" class="logs-btn">logs →</a>
  </div>
</div>
"#,
            status = s.last_status,
        ));
    }

    if cards.is_empty() {
        cards = r#"<div class="card empty">No services found in ~/.epc/services.toml</div>"#.to_string();
    }

    format!(
        r#"<!DOCTYPE html>
<html lang="en">
<head>
<meta charset="UTF-8">
<meta name="viewport" content="width=device-width, initial-scale=1, viewport-fit=cover">
<meta http-equiv="refresh" content="30">
<title>Observatory</title>
<style>
  * {{ box-sizing: border-box; margin: 0; padding: 0; }}
  body {{
    background: #0f0f1a;
    color: #e0e0f0;
    font-family: 'SF Mono', 'Menlo', monospace;
    font-size: 14px;
    padding: 16px;
    padding-bottom: calc(64px + env(safe-area-inset-bottom));
    max-width: 480px;
    margin: 0 auto;
  }}
  h1 {{
    font-size: 18px;
    font-weight: 600;
    color: #a0a0c0;
    letter-spacing: 0.05em;
    margin-bottom: 16px;
    display: flex;
    align-items: center;
    gap: 8px;
  }}
  h1 svg {{
    opacity: 0.5;
  }}
  .hex-icon polygon {{
    stroke: #a0a0c0;
    stroke-width: 1.5;
    stroke-linejoin: round;
  }}
  .card {{
    background: #1a1a2e;
    border: 1px solid #2a2a4a;
    border-radius: 10px;
    padding: 14px;
    margin-bottom: 12px;
  }}
  .card.empty {{
    color: #606080;
    font-style: italic;
  }}
  .card-header {{
    display: flex;
    justify-content: space-between;
    align-items: center;
    margin-bottom: 6px;
  }}
  .svc-name {{
    font-weight: 600;
    font-size: 15px;
    color: #c8c8f0;
  }}
  .badge {{
    font-size: 12px;
    color: #9090b0;
    display: flex;
    align-items: center;
    gap: 5px;
  }}
  .pip {{
    display: inline-block;
    width: 7px;
    height: 7px;
    border-radius: 50%;
    flex-shrink: 0;
  }}
  .pip.running  {{ background: #4caf50; box-shadow: 0 0 5px #4caf5055; }}
  .pip.degraded {{ background: #ff9800; box-shadow: 0 0 5px #ff980055; }}
  .pip.stopped  {{ background: #2a2a4a; border: 1px solid #444466; }}
  .card-meta {{
    font-size: 11px;
    color: #505070;
    margin-bottom: 8px;
  }}
  .sparkline {{
    font-size: 13px;
    letter-spacing: 1px;
    line-height: 1;
    white-space: nowrap;
    overflow: hidden;
  }}
  .dot.running  {{ color: #4caf50; }}
  .dot.degraded {{ color: #ff9800; }}
  .dot.stopped  {{ color: #444466; }}
  .ci-link {{ display: inline-block; }}
  .ci-badge {{ height: 18px; border-radius: 3px; vertical-align: middle; }}
  .card-actions {{
    display: flex;
    align-items: center;
    gap: 10px;
    margin-top: 8px;
  }}
  .svc-link {{
    color: #c8c8f0;
    text-decoration: none;
  }}
  .svc-link:hover {{ color: #a0a0ff; text-decoration: underline; }}
  .logs-btn {{
    margin-left: auto;
    font-size: 11px;
    color: #6060a0;
    text-decoration: none;
    padding: 2px 6px;
    border: 1px solid #3a3a6a;
    border-radius: 4px;
  }}
  .logs-btn:hover {{ color: #a0a0c0; border-color: #6060a0; }}
  .section-label {{
    font-size: 11px;
    color: #404060;
    letter-spacing: 0.1em;
    text-transform: uppercase;
    margin: 16px 0 8px 2px;
  }}
  .node-metric {{
    font-size: 12px;
    font-family: 'SF Mono', 'Menlo', monospace;
    margin-bottom: 4px;
  }}
  .metric-label {{
    display: inline-block;
    width: 36px;
    color: #505070;
    font-size: 11px;
  }}
  .node-services {{
    margin-top: 8px;
    display: flex;
    flex-wrap: wrap;
    gap: 8px;
  }}
  .svc-badge {{
    font-size: 11px;
    color: #8080a0;
    background: #12122a;
    border: 1px solid #2a2a4a;
    border-radius: 4px;
    padding: 2px 6px;
    text-decoration: none;
    display: inline-block;
  }}
  .svc-badge:hover {{
    border-color: #6060a0;
    color: #a0a0c0;
  }}
  .bottom-bar {{
    position: fixed;
    bottom: 0;
    left: 0;
    right: 0;
    padding: 12px 20px;
    padding-bottom: calc(12px + env(safe-area-inset-bottom));
    background: rgba(15, 15, 26, 0.65);
    -webkit-backdrop-filter: blur(24px) saturate(180%);
    backdrop-filter: blur(24px) saturate(180%);
    border-top: 1px solid rgba(160, 160, 220, 0.1);
    display: flex;
    justify-content: space-between;
    align-items: center;
  }}
  .bottom-bar .label {{
    font-size: 11px;
    color: #505070;
    letter-spacing: 0.04em;
  }}
  .map-link {{
    font-size: 11px;
    color: #6060a0;
    text-decoration: none;
    padding: 3px 8px;
    border: 1px solid #3a3a6a;
    border-radius: 4px;
  }}
  .map-link:hover {{ color: #a0a0c0; border-color: #6060a0; }}
  .bottom-bar .pulse {{
    width: 6px;
    height: 6px;
    border-radius: 50%;
    background: #4caf50;
    box-shadow: 0 0 6px #4caf50;
    animation: pulse 2s infinite;
  }}
  @keyframes pulse {{
    0%, 100% {{ opacity: 1; }}
    50% {{ opacity: 0.3; }}
  }}
</style>
</head>
<body>
<h1>
  <svg class="hex-icon" width="16" height="16" viewBox="0 0 16 16" fill="none">
    <polygon points="8,1 14.5,4.5 14.5,11.5 8,15 1.5,11.5 1.5,4.5"/>
  </svg>
  Observatory
</h1>
{node_section}
{cards}
<div class="bottom-bar">
  <span class="label">Observatory &nbsp;·&nbsp; refreshes every 30s</span>
  <a href="/map" class="map-link">map →</a>
  <div class="pulse"></div>
</div>
</body>
</html>
"#
    )
}
