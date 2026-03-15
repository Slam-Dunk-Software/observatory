use std::path::PathBuf;

use serde::Deserialize;
use tokio::process::Command;

// ── Config ────────────────────────────────────────────────────────────────────

#[derive(Debug, Deserialize, Clone)]
pub struct NodeConfig {
    pub name: String,
    pub host: String,
    pub ssh_user: String,
    pub ssh_key: Option<String>,
    #[serde(default)]
    pub services: Vec<String>,
    #[serde(default = "default_disk_threshold")]
    pub disk_alert_threshold: u8,
    #[serde(default = "default_cpu_threshold")]
    pub cpu_alert_threshold: f64,
}

fn default_disk_threshold() -> u8 { 85 }
fn default_cpu_threshold() -> f64 { 95.0 }

#[derive(Debug, Deserialize)]
struct NodesFile {
    #[serde(default)]
    nodes: Vec<NodeConfig>,
}

pub fn nodes_toml_path() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("/tmp"))
        .join(".epc/nodes.toml")
}

pub fn load_nodes() -> Vec<NodeConfig> {
    let path = nodes_toml_path();
    let content = match std::fs::read_to_string(&path) {
        Ok(c) => c,
        Err(_) => return vec![],
    };
    let file: NodesFile = match toml::from_str(&content) {
        Ok(f) => f,
        Err(e) => {
            eprintln!("[observatory] failed to parse nodes.toml: {e}");
            return vec![];
        }
    };
    file.nodes
}

// ── Metrics ───────────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct NodeMetrics {
    pub disk_pct: Option<u8>,
    pub cpu_load: Option<f64>,
    pub mem_pct: Option<u8>,
    pub service_statuses: Vec<(String, bool)>, // (service_name, is_active)
    pub status: String, // "ok", "warn", "alert", "unreachable"
}

pub async fn poll_node(node: &NodeConfig) -> NodeMetrics {
    let svc_checks = node
        .services
        .iter()
        .map(|s| {
            format!(
                "echo SVC:{}:$(systemctl is-active {} 2>/dev/null || echo inactive)",
                s, s
            )
        })
        .collect::<Vec<_>>()
        .join("; ");

    let cmd = format!(
        "echo DISK:$(df -P / | tail -1 | awk '{{print $5}}' | tr -d '%'); \
         echo LOAD:$(cat /proc/loadavg | awk '{{print $1}}'); \
         echo MEM:$(free -m | grep Mem | awk '{{printf \"%.0f\", $3/$2*100}}'); \
         {svc_checks}"
    );

    let key_path = resolve_key(node.ssh_key.as_deref().unwrap_or("~/.ssh/id_ed25519"));

    let output = Command::new("ssh")
        .args([
            "-i",
            &key_path,
            "-o",
            "StrictHostKeyChecking=no",
            "-o",
            "ConnectTimeout=5",
            "-o",
            "BatchMode=yes",
            &format!("{}@{}", node.ssh_user, node.host),
            &cmd,
        ])
        .output()
        .await;

    match output {
        Err(e) => {
            eprintln!("[observatory] SSH spawn failed for {}: {e}", node.name);
            unreachable_metrics()
        }
        Ok(o) if !o.status.success() => {
            eprintln!(
                "[observatory] SSH failed for {}: {}",
                node.name,
                String::from_utf8_lossy(&o.stderr).trim()
            );
            unreachable_metrics()
        }
        Ok(o) => parse_output(&String::from_utf8_lossy(&o.stdout), node),
    }
}

fn resolve_key(key: &str) -> String {
    if key.starts_with('~') {
        if let Some(home) = dirs::home_dir() {
            return format!("{}{}", home.display(), &key[1..]);
        }
    }
    key.to_string()
}

fn unreachable_metrics() -> NodeMetrics {
    NodeMetrics {
        disk_pct: None,
        cpu_load: None,
        mem_pct: None,
        service_statuses: vec![],
        status: "unreachable".into(),
    }
}

fn parse_output(output: &str, node: &NodeConfig) -> NodeMetrics {
    let mut disk_pct: Option<u8> = None;
    let mut cpu_load: Option<f64> = None;
    let mut mem_pct: Option<u8> = None;
    let mut service_statuses: Vec<(String, bool)> = vec![];

    for line in output.lines() {
        if let Some(val) = line.strip_prefix("DISK:") {
            disk_pct = val.trim().parse().ok();
        } else if let Some(val) = line.strip_prefix("LOAD:") {
            cpu_load = val.trim().parse().ok();
        } else if let Some(val) = line.strip_prefix("MEM:") {
            mem_pct = val.trim().parse().ok();
        } else if let Some(rest) = line.strip_prefix("SVC:") {
            if let Some((name, state)) = rest.split_once(':') {
                service_statuses.push((name.to_string(), state.trim() == "active"));
            }
        }
    }

    let status = compute_status(&disk_pct, &cpu_load, node);
    NodeMetrics { disk_pct, cpu_load, mem_pct, service_statuses, status }
}

fn compute_status(disk_pct: &Option<u8>, cpu_load: &Option<f64>, node: &NodeConfig) -> String {
    if disk_pct.is_none() && cpu_load.is_none() {
        return "unreachable".into();
    }
    if let Some(&d) = disk_pct.as_ref() {
        if d >= node.disk_alert_threshold {
            return "alert".into();
        }
        if d >= node.disk_alert_threshold.saturating_sub(10) {
            return "warn".into();
        }
    }
    if let Some(&l) = cpu_load.as_ref() {
        if l >= node.cpu_alert_threshold {
            return "alert".into();
        }
    }
    "ok".into()
}
