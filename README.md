# Observatory

Health dashboard for EPS services. Polls `~/.epc/services.toml` on an interval, stores results in `~/.epc/observatory.db`, and serves a live status page.

## Known Issues

**Stale service entries** — Observatory persists service state in its own SQLite database (`~/.epc/observatory.db`). If a service is removed (deleted project, `epm services stop` + manual removal from `services.toml`), Observatory has no way to know — it will continue showing that service with its last known status indefinitely.

The right fix would be a reconciliation pass that drops DB entries for services no longer in `services.toml`, but that's risky: it would silently delete historical health data and logs for services that are just temporarily stopped or being debugged. For now, stale entries have to be cleaned up manually:

```sh
sqlite3 ~/.epc/observatory.db "DELETE FROM service_state WHERE service = 'name'; DELETE FROM health_checks WHERE service = 'name';"
```
