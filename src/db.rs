use std::collections::HashMap;

use anyhow::Result;
use rusqlite::{Connection, params};

pub fn init(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS health_checks (
            id          INTEGER PRIMARY KEY AUTOINCREMENT,
            service     TEXT NOT NULL,
            checked_at  TEXT NOT NULL,
            status      TEXT NOT NULL,
            response_ms INTEGER,
            status_code INTEGER
        );
        CREATE TABLE IF NOT EXISTS service_state (
            service      TEXT PRIMARY KEY,
            last_status  TEXT NOT NULL,
            last_checked TEXT NOT NULL
        );",
    )?;
    // Migration: add repo_url column (no-op if already present)
    conn.execute_batch("ALTER TABLE service_state ADD COLUMN repo_url TEXT").ok();

    // Node monitoring tables
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS node_checks (
            id          INTEGER PRIMARY KEY AUTOINCREMENT,
            node        TEXT NOT NULL,
            checked_at  TEXT NOT NULL,
            disk_pct    INTEGER,
            cpu_load    REAL,
            mem_pct     INTEGER,
            status      TEXT NOT NULL
        );
        CREATE TABLE IF NOT EXISTS node_state (
            node         TEXT PRIMARY KEY,
            last_status  TEXT NOT NULL,
            last_checked TEXT NOT NULL,
            disk_pct     INTEGER,
            cpu_load     REAL,
            mem_pct      INTEGER
        );
        CREATE TABLE IF NOT EXISTS node_service_checks (
            id          INTEGER PRIMARY KEY AUTOINCREMENT,
            node        TEXT NOT NULL,
            service     TEXT NOT NULL,
            checked_at  TEXT NOT NULL,
            active      INTEGER NOT NULL
        );",
    )?;
    Ok(())
}

pub fn insert_check(
    conn: &Connection,
    service: &str,
    checked_at: &str,
    status: &str,
    response_ms: Option<i64>,
    status_code: Option<u16>,
) -> Result<()> {
    conn.execute(
        "INSERT INTO health_checks (service, checked_at, status, response_ms, status_code)
         VALUES (?1, ?2, ?3, ?4, ?5)",
        params![service, checked_at, status, response_ms, status_code.map(|c| c as i64)],
    )?;
    Ok(())
}

/// Returns the last_status for a service, or None if not seen before.
pub fn get_last_status(conn: &Connection, service: &str) -> Result<Option<String>> {
    let mut stmt = conn.prepare(
        "SELECT last_status FROM service_state WHERE service = ?1",
    )?;
    let mut rows = stmt.query(params![service])?;
    if let Some(row) = rows.next()? {
        Ok(Some(row.get(0)?))
    } else {
        Ok(None)
    }
}

pub fn set_last_status(
    conn: &Connection,
    service: &str,
    status: &str,
    checked_at: &str,
    repo_url: Option<&str>,
) -> Result<()> {
    conn.execute(
        "INSERT INTO service_state (service, last_status, last_checked, repo_url)
         VALUES (?1, ?2, ?3, ?4)
         ON CONFLICT(service) DO UPDATE SET last_status = ?2, last_checked = ?3, repo_url = ?4",
        params![service, status, checked_at, repo_url],
    )?;
    Ok(())
}

#[allow(dead_code)]
pub struct CheckRow {
    pub status: String,
    pub response_ms: Option<i64>,
    pub checked_at: String,
}

/// Returns the last `limit` checks for a service, newest first.
pub fn recent_checks(conn: &Connection, service: &str, limit: usize) -> Result<Vec<CheckRow>> {
    let mut stmt = conn.prepare(
        "SELECT status, response_ms, checked_at
         FROM health_checks
         WHERE service = ?1
         ORDER BY id DESC
         LIMIT ?2",
    )?;
    let rows = stmt.query_map(params![service, limit as i64], |row| {
        Ok(CheckRow {
            status: row.get(0)?,
            response_ms: row.get(1)?,
            checked_at: row.get(2)?,
        })
    })?;
    let mut out = Vec::new();
    for r in rows {
        out.push(r?);
    }
    Ok(out)
}

/// Snapshot of current state for all services.
pub struct ServiceSnapshot {
    pub service: String,
    pub last_status: String,
    pub last_checked: String,
    pub repo_url: Option<String>,
}

pub fn all_states(conn: &Connection) -> Result<Vec<ServiceSnapshot>> {
    let mut stmt = conn.prepare(
        "SELECT service, last_status, last_checked, repo_url FROM service_state ORDER BY service",
    )?;
    let rows = stmt.query_map([], |row| {
        Ok(ServiceSnapshot {
            service: row.get(0)?,
            last_status: row.get(1)?,
            last_checked: row.get(2)?,
            repo_url: row.get(3)?,
        })
    })?;
    let mut out = Vec::new();
    for r in rows {
        out.push(r?);
    }
    Ok(out)
}

// ── Node monitoring ───────────────────────────────────────────────────────────

pub fn insert_node_check(
    conn: &Connection,
    node: &str,
    checked_at: &str,
    disk_pct: Option<u8>,
    cpu_load: Option<f64>,
    mem_pct: Option<u8>,
    status: &str,
) -> Result<()> {
    conn.execute(
        "INSERT INTO node_checks (node, checked_at, disk_pct, cpu_load, mem_pct, status)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        params![node, checked_at, disk_pct.map(|v| v as i64), cpu_load, mem_pct.map(|v| v as i64), status],
    )?;
    Ok(())
}

pub fn get_node_last_status(conn: &Connection, node: &str) -> Result<Option<String>> {
    let mut stmt = conn.prepare("SELECT last_status FROM node_state WHERE node = ?1")?;
    let mut rows = stmt.query(params![node])?;
    if let Some(row) = rows.next()? {
        Ok(Some(row.get(0)?))
    } else {
        Ok(None)
    }
}

pub fn set_node_state(
    conn: &Connection,
    node: &str,
    status: &str,
    checked_at: &str,
    disk_pct: Option<u8>,
    cpu_load: Option<f64>,
    mem_pct: Option<u8>,
) -> Result<()> {
    conn.execute(
        "INSERT INTO node_state (node, last_status, last_checked, disk_pct, cpu_load, mem_pct)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)
         ON CONFLICT(node) DO UPDATE SET
           last_status = ?2, last_checked = ?3,
           disk_pct = ?4, cpu_load = ?5, mem_pct = ?6",
        params![node, status, checked_at, disk_pct.map(|v| v as i64), cpu_load, mem_pct.map(|v| v as i64)],
    )?;
    Ok(())
}

pub fn insert_node_service_check(
    conn: &Connection,
    node: &str,
    service: &str,
    checked_at: &str,
    active: bool,
) -> Result<()> {
    conn.execute(
        "INSERT INTO node_service_checks (node, service, checked_at, active) VALUES (?1, ?2, ?3, ?4)",
        params![node, service, checked_at, active as i64],
    )?;
    Ok(())
}

pub struct NodeSnapshot {
    pub node: String,
    pub last_status: String,
    pub last_checked: String,
    pub disk_pct: Option<u8>,
    pub cpu_load: Option<f64>,
    pub mem_pct: Option<u8>,
}

pub fn all_node_states(conn: &Connection) -> Result<Vec<NodeSnapshot>> {
    let mut stmt = conn.prepare(
        "SELECT node, last_status, last_checked, disk_pct, cpu_load, mem_pct
         FROM node_state ORDER BY node",
    )?;
    let rows = stmt.query_map([], |row| {
        Ok(NodeSnapshot {
            node: row.get(0)?,
            last_status: row.get(1)?,
            last_checked: row.get(2)?,
            disk_pct: row.get::<_, Option<i64>>(3)?.map(|v| v as u8),
            cpu_load: row.get(4)?,
            mem_pct: row.get::<_, Option<i64>>(5)?.map(|v| v as u8),
        })
    })?;
    let mut out = Vec::new();
    for r in rows {
        out.push(r?);
    }
    Ok(out)
}

/// Returns latest active status per service per node: node → service → is_active
pub fn latest_node_service_statuses(
    conn: &Connection,
) -> Result<HashMap<String, HashMap<String, bool>>> {
    let mut stmt = conn.prepare(
        "SELECT node, service, active FROM node_service_checks
         WHERE id IN (SELECT MAX(id) FROM node_service_checks GROUP BY node, service)",
    )?;
    let rows = stmt.query_map([], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, i64>(2)?,
        ))
    })?;
    let mut map: HashMap<String, HashMap<String, bool>> = HashMap::new();
    for row in rows {
        let (node, service, active) = row?;
        map.entry(node).or_default().insert(service, active != 0);
    }
    Ok(map)
}
