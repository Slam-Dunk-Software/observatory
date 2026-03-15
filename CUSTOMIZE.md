# observatory — Customization Guide

Observatory is a lightweight health monitoring dashboard for EPC services. It polls the
health endpoints of every service in `~/.epc/services.toml`, stores history in SQLite,
and serves a mobile-friendly dark-mode dashboard with status badges and dot-grid sparklines.
It also fires txtme alerts when a service transitions between running/degraded/stopped.

## Ports

### `POLL_INTERVAL_SECS`

**What it does:** How often Observatory polls each service's health endpoint.
**Default:** `30` seconds
**How to customize:** Set `POLL_INTERVAL_SECS=60` in `.env` (or your shell environment before running).

### `TXTME_URL`

**What it does:** The URL Observatory POSTs to when a service changes status. Set this
to your txtme endpoint to receive SMS alerts on state transitions.
**Default:** Not set — alerts are silently skipped if absent.
**How to customize:** Add `TXTME_URL=http://<your-txtme-host>:5543` to `.env`.
Alert messages look like: `[Observatory] morning_brief is DOWN` / `[Observatory] txtme recovered`.

### `PORT`

**What it does:** The port Observatory's web server binds to.
**Default:** `9090`
**How to customize:** Set `PORT=8888` in `.env` or in `eps.toml [service] port`.

### `MAP_PATH` — code map file

**What it does:** Path to a Markdown file served at `/map`. Useful for displaying
a generated code map (e.g. from `tree_walker`).
**Default:** `~/.epc/code_map.md`
**How to customize:** Set `MAP_PATH=/path/to/your/map.md` in `.env`.
The `/map/refresh` endpoint runs `tree_walker --output <MAP_PATH>` to regenerate it.

---

### `DASHBOARD_SPARKLINE_LENGTH`

**What it does:** Number of recent checks shown in the dot-grid per service.
**Default:** `40` dots
**How to customize:** Edit `src/dashboard.rs` line `db::recent_checks(&conn, &s.service, 40)` — change
`40` to your preferred history depth. Rebuild with `cargo build --release`.

## Getting Started

1. Copy the example env: `cp .env.example .env`
2. Set `TXTME_URL` in `.env` if you want SMS alerts
3. Build: `cargo build --release`
4. Run: `observatory serve`
5. Open `http://localhost:9090` (or your Tailscale IP at port 9090) in a browser

## Common Customizations

### Adding alert thresholds (e.g. only alert after 3 consecutive failures)

Edit `src/poller.rs` — in `poll_once()`, before calling `alert::send()`, query the last N
rows from `health_checks` and only fire if all N are the same non-running status.

### Changing the dashboard refresh interval

Edit `src/dashboard.rs` — find `<meta http-equiv="refresh" content="30">` and change `30`
to your preferred refresh interval in seconds.

### Monitoring services not managed by EPC

Observatory reads `~/.epc/services.toml` for service discovery. To monitor additional
endpoints, edit `src/poller.rs` → `poll_once()` and add entries to a hardcoded list
alongside the services discovered from the state file.
