use std::{
    collections::HashMap,
    path::PathBuf,
    process::Command,
    sync::{Arc, Mutex},
    time::Duration,
};

use anyhow::Result;
use chrono::Utc;
use rusqlite::Connection;
use serde::Deserialize;
use tokio::time;

use crate::{alert, db, nodes};

// ── EPC state file structs ────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
struct ServiceEntry {
    dir: String,
    port: u16,
}

#[derive(Debug, Deserialize)]
struct ServicesFile {
    #[serde(default)]
    services: HashMap<String, ServiceEntry>,
}

// ── eps.toml minimal parser ───────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
struct EpsPackage {
    repository: Option<String>,
}

#[derive(Debug, Deserialize)]
struct EpsService {
    health_check: Option<String>,
    tls: Option<bool>,
}

#[derive(Debug, Deserialize)]
struct EpsManifest {
    package: Option<EpsPackage>,
    service: Option<EpsService>,
}

/// Returns (health_check, tls, repo_url) from eps.toml.
fn read_eps_info(dir: &str) -> (Option<String>, Option<bool>, Option<String>) {
    let path = PathBuf::from(dir).join("eps.toml");
    let content = match std::fs::read_to_string(path) {
        Ok(c) => c,
        Err(_) => return (None, None, None),
    };
    let manifest: EpsManifest = match toml::from_str(&content) {
        Ok(m) => m,
        Err(_) => return (None, None, None),
    };
    let tls = manifest.service.as_ref().and_then(|s| s.tls);
    let health_check = manifest.service.and_then(|s| s.health_check);
    let repo_url = manifest.package.and_then(|p| p.repository);
    (health_check, tls, repo_url)
}

// ── Port-listening check (mirrors EPC's approach) ─────────────────────────────

fn is_port_listening(port: u16) -> bool {
    let out = Command::new("lsof")
        .args(["-t", "-i", &format!(":{port}"), "-sTCP:LISTEN"])
        .output();
    match out {
        Ok(o) => !o.stdout.is_empty(),
        Err(_) => false,
    }
}

// ── Tailscale IP ──────────────────────────────────────────────────────────────

fn tailscale_ip() -> String {
    let out = Command::new("tailscale")
        .args(["ip", "-4"])
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .unwrap_or_default();
    out.trim().to_string()
}

// ── EPC state file path ───────────────────────────────────────────────────────

fn services_toml_path() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("/tmp"))
        .join(".epc/services.toml")
}

// ── Main poll loop ────────────────────────────────────────────────────────────

pub async fn run(
    db: Arc<Mutex<Connection>>,
    http: reqwest::Client,
    txtme_url: Option<String>,
    interval_secs: u64,
) {
    let mut ticker = time::interval(Duration::from_secs(interval_secs));
    loop {
        ticker.tick().await;
        if let Err(e) = poll_once(&db, &http, txtme_url.as_deref()).await {
            eprintln!("[observatory] poll error: {e}");
        }
    }
}

async fn poll_once(
    db: &Arc<Mutex<Connection>>,
    http: &reqwest::Client,
    txtme_url: Option<&str>,
) -> Result<()> {
    let content = std::fs::read_to_string(services_toml_path())?;
    let file: ServicesFile = toml::from_str(&content)?;
    let ts_ip = tailscale_ip();

    for (name, entry) in &file.services {
        let (status, response_ms, status_code, repo_url) =
            check_service(http, name, entry, &ts_ip).await;

        let now = Utc::now().to_rfc3339();

        let prev = {
            let conn = db.lock().unwrap();
            db::get_last_status(&conn, name).unwrap_or(None)
        };

        {
            let conn = db.lock().unwrap();
            db::insert_check(&conn, name, &now, &status, response_ms, status_code).ok();
            db::set_last_status(&conn, name, &status, &now, repo_url.as_deref()).ok();
        }

        // Alert on transition
        if let Some(prev_status) = prev {
            if prev_status != status {
                if let Some(url) = txtme_url {
                    let msg = match status.as_str() {
                        "running" => format!("[Observatory] {name} recovered"),
                        "degraded" => format!("[Observatory] {name} is DEGRADED"),
                        _ => format!("[Observatory] {name} is DOWN"),
                    };
                    if let Err(e) = alert::send(http, url, &msg).await {
                        eprintln!("[observatory] alert failed for {name}: {e}");
                    }
                }
            }
        }
    }

    // ── Node polling ──────────────────────────────────────────────────────────
    let node_list = nodes::load_nodes();
    for node in &node_list {
        let metrics = nodes::poll_node(node).await;
        let now = Utc::now().to_rfc3339();

        let prev = {
            let conn = db.lock().unwrap();
            db::get_node_last_status(&conn, &node.name).unwrap_or(None)
        };

        {
            let conn = db.lock().unwrap();
            db::insert_node_check(
                &conn, &node.name, &now,
                metrics.disk_pct, metrics.cpu_load, metrics.mem_pct, &metrics.status,
            ).ok();
            db::set_node_state(
                &conn, &node.name, &metrics.status, &now,
                metrics.disk_pct, metrics.cpu_load, metrics.mem_pct,
            ).ok();
            for (svc, active) in &metrics.service_statuses {
                db::insert_node_service_check(&conn, &node.name, svc, &now, *active).ok();
            }
        }

        // Alert: node went unreachable
        if let Some(ref prev_status) = prev {
            if prev_status != "unreachable" && metrics.status == "unreachable" {
                if let Some(url) = txtme_url {
                    let msg = format!("[Observatory] {} is UNREACHABLE", node.name);
                    alert::send(http, url, &msg).await.ok();
                }
            } else if prev_status == "unreachable" && metrics.status != "unreachable" {
                if let Some(url) = txtme_url {
                    let msg = format!("[Observatory] {} is back online", node.name);
                    alert::send(http, url, &msg).await.ok();
                }
            }
        }

        // Alert: threshold crossings
        if let Some(url) = txtme_url {
            if let Some(d) = metrics.disk_pct {
                if d >= node.disk_alert_threshold {
                    let msg = format!("[Observatory] {}: disk {}% full", node.name, d);
                    alert::send(http, url, &msg).await.ok();
                }
            }
            if let Some(l) = metrics.cpu_load {
                if l >= node.cpu_alert_threshold {
                    let msg = format!("[Observatory] {}: CPU load {:.2}", node.name, l);
                    alert::send(http, url, &msg).await.ok();
                }
            }
        }

        eprintln!(
            "[observatory] node {} → {} (disk={:?}% cpu={:?} mem={:?}%)",
            node.name, metrics.status, metrics.disk_pct, metrics.cpu_load, metrics.mem_pct
        );
    }

    Ok(())
}

async fn check_service(
    http: &reqwest::Client,
    name: &str,
    entry: &ServiceEntry,
    ts_ip: &str,
) -> (String, Option<i64>, Option<u16>, Option<String>) {
    let (health_check, tls, repo_url) = read_eps_info(&entry.dir);

    if !is_port_listening(entry.port) {
        return ("stopped".into(), None, None, repo_url);
    }

    if health_check.is_none() {
        return ("running".into(), None, None, repo_url);
    }

    let use_tls = tls.unwrap_or(false);
    let scheme = if use_tls { "https" } else { "http" };
    let url = format!("{}://{}:{}/health", scheme, ts_ip, entry.port);
    let tls_client;
    let client: &reqwest::Client = if use_tls {
        tls_client = reqwest::Client::builder()
            .danger_accept_invalid_certs(true)
            .timeout(Duration::from_secs(2))
            .build()
            .unwrap_or_else(|_| http.clone());
        &tls_client
    } else {
        http
    };
    let start = std::time::Instant::now();
    match client
        .get(&url)
        .timeout(Duration::from_secs(2))
        .send()
        .await
    {
        Ok(resp) => {
            let ms = start.elapsed().as_millis() as i64;
            let code = resp.status().as_u16();
            if resp.status().is_success() {
                ("running".into(), Some(ms), Some(code), repo_url)
            } else {
                eprintln!("[observatory] {name} health returned {code}");
                ("degraded".into(), Some(ms), Some(code), repo_url)
            }
        }
        Err(e) => {
            let ms = start.elapsed().as_millis() as i64;
            eprintln!("[observatory] {name} health check failed: {e}");
            ("degraded".into(), Some(ms), None, repo_url)
        }
    }
}
