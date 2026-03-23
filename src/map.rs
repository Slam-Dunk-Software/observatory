use axum::{
    http::{header, HeaderMap, StatusCode},
    response::IntoResponse,
};
use pulldown_cmark::{html, Options, Parser};

fn map_path() -> std::path::PathBuf {
    std::env::var("MAP_PATH")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|_| {
            dirs::home_dir()
                .unwrap_or_else(|| std::path::PathBuf::from("/tmp"))
                .join(".epm/services/code_map.md")
        })
}

fn markdown_to_html(md: &str) -> String {
    let parser = Parser::new_ext(md, Options::empty());
    let mut out = String::new();
    html::push_html(&mut out, parser);
    out
}

fn render_page(body_html: &str, refreshing: bool) -> String {
    let status_msg = if refreshing {
        r#"<div class="status refreshing">refreshing…</div>"#
    } else {
        ""
    };

    format!(
        r#"<!DOCTYPE html>
<html lang="en">
<head>
  <meta charset="UTF-8">
  <meta name="viewport" content="width=device-width, initial-scale=1, viewport-fit=cover">
  <title>EPS Map — Observatory</title>
  <style>
    * {{ box-sizing: border-box; margin: 0; padding: 0; }}
    body {{
      background: #0f0f1a;
      color: #c8c8e8;
      font-family: 'SF Mono', 'Menlo', monospace;
      font-size: 13px;
      padding: 16px;
      padding-bottom: calc(64px + env(safe-area-inset-bottom));
      max-width: 760px;
      margin: 0 auto;
    }}
    .header {{
      display: flex;
      align-items: center;
      gap: 12px;
      margin-bottom: 20px;
    }}
    a.back {{
      color: #6060a0;
      text-decoration: none;
      font-size: 12px;
      flex-shrink: 0;
    }}
    a.back:hover {{ color: #a0a0c0; }}
    h1 {{
      font-size: 16px;
      font-weight: 600;
      color: #a0a0c0;
      letter-spacing: 0.05em;
    }}
    /* markdown content */
    .content h1 {{
      font-size: 18px;
      color: #c8c8f0;
      margin-bottom: 4px;
      letter-spacing: 0.04em;
    }}
    .content p em {{
      color: #505070;
      font-style: normal;
      font-size: 12px;
    }}
    .content hr {{
      border: none;
      border-top: 1px solid #2a2a4a;
      margin: 16px 0;
    }}
    .content h2 {{
      font-size: 14px;
      font-weight: 600;
      color: #a0a0d0;
      margin: 20px 0 8px;
      letter-spacing: 0.04em;
    }}
    .content h2 code {{
      font-size: 11px;
      color: #505080;
      background: none;
      padding: 0;
      border: none;
    }}
    .content p {{
      line-height: 1.7;
      color: #9090b0;
      margin-bottom: 2px;
    }}
    .content p code {{
      color: #a0c0e0;
      background: #12122a;
      border: 1px solid #2a2a4a;
      border-radius: 3px;
      padding: 0 4px;
      font-size: 12px;
    }}
    .status.refreshing {{
      font-size: 12px;
      color: #ff9800;
      margin-left: auto;
    }}
    .bottom-bar {{
      position: fixed;
      bottom: 0;
      left: 0;
      right: 0;
      padding: 12px 20px;
      padding-bottom: calc(12px + env(safe-area-inset-bottom));
      background: rgba(15, 15, 26, 0.85);
      -webkit-backdrop-filter: blur(24px) saturate(180%);
      backdrop-filter: blur(24px) saturate(180%);
      border-top: 1px solid rgba(160, 160, 220, 0.1);
      display: flex;
      justify-content: space-between;
      align-items: center;
    }}
    .refresh-form {{ display: inline; }}
    .refresh-btn {{
      background: none;
      border: 1px solid #3a3a6a;
      color: #6060a0;
      font-family: 'SF Mono', 'Menlo', monospace;
      font-size: 12px;
      padding: 4px 10px;
      border-radius: 5px;
      cursor: pointer;
    }}
    .refresh-btn:hover {{ color: #a0a0c0; border-color: #6060a0; }}
    .bottom-label {{
      font-size: 11px;
      color: #505070;
      letter-spacing: 0.04em;
    }}
  </style>
</head>
<body>
  <div class="header">
    <a class="back" href="/">← Observatory</a>
    {status_msg}
  </div>
  <div class="content">
    {body_html}
  </div>
  <div class="bottom-bar">
    <span class="bottom-label">EPS Code Map</span>
    <form class="refresh-form" method="POST" action="/map/refresh">
      <button class="refresh-btn" type="submit">↺ refresh</button>
    </form>
  </div>
</body>
</html>"#
    )
}

pub async fn handler() -> impl IntoResponse {
    let path = map_path();
    let md = match std::fs::read_to_string(&path) {
        Ok(s) => s,
        Err(_) => format!(
            "# EPS Code Map\n\nNo map found at `{}`.\n\nRun `tree_walker -o {}` to generate it.",
            path.display(),
            path.display()
        ),
    };

    let body = markdown_to_html(&md);
    let html = render_page(&body, false);

    let mut headers = HeaderMap::new();
    headers.insert(
        header::CONTENT_TYPE,
        "text/html; charset=utf-8".parse().unwrap(),
    );
    (StatusCode::OK, headers, html).into_response()
}

pub async fn refresh_handler() -> impl IntoResponse {
    let map_path = map_path();
    let tree_walker = dirs::home_dir()
        .unwrap_or_default()
        .join(".cargo/bin/tree_walker");

    // Try ~/.cargo/bin/tree_walker, fall back to PATH
    let bin = if tree_walker.exists() {
        tree_walker.to_string_lossy().into_owned()
    } else {
        "tree_walker".to_string()
    };

    let _ = tokio::process::Command::new(&bin)
        .args(["--output", &map_path.to_string_lossy()])
        .status()
        .await;

    axum::response::Redirect::to("/map")
}
