mod alert;
mod dashboard;
mod db;
mod map;
mod nodes;
mod poller;
mod tui;

use std::{
    path::PathBuf,
    sync::{Arc, Mutex},
};

use anyhow::Result;
use axum::{Router, routing::{get, post}, http::{header, HeaderMap, StatusCode}, response::IntoResponse};
use clap::{Parser, Subcommand};
use tower_http::trace::TraceLayer;
use rusqlite::Connection;

#[derive(Parser)]
#[command(name = "observatory", about = "EPS health monitoring dashboard")]
struct Cli {
    #[command(subcommand)]
    command: Option<Commands>,
}

#[derive(Subcommand)]
enum Commands {
    /// Start the web dashboard server (default)
    Serve,
    /// Open the terminal UI
    Tui,
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Some(Commands::Tui) => return tui::run(),
        _ => {} // fall through to serve
    }

    tracing_subscriber::fmt::init();
    // Load .env if present
    dotenvy::dotenv().ok();

    let host = std::env::var("HOST").unwrap_or_else(|_| "127.0.0.1".to_string());
    let port: u16 = std::env::var("PORT")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(9090);
    let txtme_url = std::env::var("TXTME_URL").ok();
    let interval_secs: u64 = std::env::var("POLL_INTERVAL_SECS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(30);

    // Open SQLite
    let db_path = dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("/tmp"))
        .join(".epm/services/observatory.db");
    if let Some(parent) = db_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let conn = Connection::open(&db_path)?;
    db::init(&conn)?;
    let db = Arc::new(Mutex::new(conn));

    // HTTP client for health checks + alerts
    let http = reqwest::Client::new();

    // Spawn poller background task
    {
        let db2 = Arc::clone(&db);
        let http2 = http.clone();
        tokio::spawn(async move {
            poller::run(db2, http2, txtme_url, interval_secs).await;
        });
    }

    // Axum router
    let app = Router::new()
        .route("/", get(dashboard::handler))
        .route("/health", get(|| async { "ok" }))
        .route("/api/services", get(api_services))
        .route("/logs/:service", get(logs_handler))
        .route("/logs/node/:node/:service", get(node_logs_handler))
        .route("/map", get(map::handler))
        .route("/map/refresh", post(map::refresh_handler))
        .layer(TraceLayer::new_for_http())
        .with_state(db);

    let addr = format!("{host}:{port}");
    println!("[observatory] listening on http://{addr}");
    let listener = tokio::net::TcpListener::bind(&addr).await?;
    axum::serve(listener, app).await?;

    Ok(())
}

async fn logs_handler(
    axum::extract::Path(service): axum::extract::Path<String>,
) -> impl IntoResponse {
    // Sanitize: prevent path traversal
    if !service.chars().all(|c| c.is_alphanumeric() || c == '_' || c == '-') {
        return (StatusCode::BAD_REQUEST, HeaderMap::new(), "invalid service name".to_string())
            .into_response();
    }
    let log_path = dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("/tmp"))
        .join(".epm/services/logs")
        .join(format!("{service}.log"));

    let content = std::fs::read_to_string(&log_path)
        .unwrap_or_else(|_| format!("No log file found at {}", log_path.display()));

    let html = dashboard::render_log_page(&service, &content);
    let mut headers = HeaderMap::new();
    headers.insert(header::CONTENT_TYPE, "text/html; charset=utf-8".parse().unwrap());
    (StatusCode::OK, headers, html).into_response()
}

async fn node_logs_handler(
    axum::extract::Path((node, service)): axum::extract::Path<(String, String)>,
) -> impl IntoResponse {
    // Sanitize both path segments
    let valid = |s: &str| s.chars().all(|c| c.is_alphanumeric() || c == '_' || c == '-');
    if !valid(&node) || !valid(&service) {
        return (StatusCode::BAD_REQUEST, HeaderMap::new(), "invalid name".to_string())
            .into_response();
    }

    let nodes = nodes::load_nodes();
    let node_cfg = match nodes.iter().find(|n| n.name == node) {
        Some(n) => n,
        None => {
            return (StatusCode::NOT_FOUND, HeaderMap::new(), format!("node '{node}' not found"))
                .into_response()
        }
    };

    let key_path = if let Some(k) = &node_cfg.ssh_key {
        if k.starts_with('~') {
            format!(
                "{}{}",
                dirs::home_dir().unwrap_or_default().display(),
                &k[1..]
            )
        } else {
            k.clone()
        }
    } else {
        format!("{}/.ssh/id_ed25519", dirs::home_dir().unwrap_or_default().display())
    };

    let cmd = format!("journalctl -u {service} --no-pager -n 500 --output=short 2>&1");
    let output = tokio::process::Command::new("ssh")
        .args([
            "-i", &key_path,
            "-o", "StrictHostKeyChecking=no",
            "-o", "ConnectTimeout=5",
            "-o", "BatchMode=yes",
            &format!("{}@{}", node_cfg.ssh_user, node_cfg.host),
            &cmd,
        ])
        .output()
        .await;

    let content = match output {
        Ok(o) => {
            let stdout = String::from_utf8_lossy(&o.stdout).to_string();
            if stdout.is_empty() {
                String::from_utf8_lossy(&o.stderr).to_string()
            } else {
                stdout
            }
        }
        Err(e) => format!("SSH error: {e}"),
    };

    let title = format!("{node} / {service}");
    let html = dashboard::render_log_page(&title, &content);
    let mut headers = HeaderMap::new();
    headers.insert(header::CONTENT_TYPE, "text/html; charset=utf-8".parse().unwrap());
    (StatusCode::OK, headers, html).into_response()
}

async fn api_services(
    axum::extract::State(db): axum::extract::State<Arc<Mutex<Connection>>>,
) -> axum::Json<serde_json::Value> {
    let conn = db.lock().unwrap();
    let states = db::all_states(&conn).unwrap_or_default();
    let arr: Vec<_> = states
        .iter()
        .map(|s| {
            serde_json::json!({
                "service": s.service,
                "status": s.last_status,
                "last_checked": s.last_checked,
            })
        })
        .collect();
    axum::Json(serde_json::json!(arr))
}
