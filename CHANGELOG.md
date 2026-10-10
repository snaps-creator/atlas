# Changelog

## 2.5.1 (unpublished)

- Combine URL subscriptions and manual VLESS sources in one repository, Add source dialog and server list; preserve source-scoped identity and selection across migration and refresh.
- Persist connection intent before starting VPN, keep visibility independent, and re-arm bounded retries after a confirmed network change. Source refresh no longer owns a reconnect loop.
- Schedule fair bounded reserve probes without requiring a manual ping. Keep healthy connections stable; latency optimization requires explicit opt-in.
- Reject unauthorized IPC clients without tearing down a surviving session, retain a bounded owner admission deadline and record queue/reply/core timings.
- Journal Windows credential compensation, retain configuration checkpoints on source errors, expose startup registration failures and inspect registered offline user hives during uninstall.
- Coalesce repeated incident failures, record build/session identity and avoid serializing the whole history on every append.
- Produce one full offline installer, `Atlas-Setup-2.5.1.exe`, distributed only as a GitHub Releases asset. Replace tracked legacy installer parts with signed CI acceptance; require separate manual authorization for publication.

Actual evidence and remaining release gates: [2.5.1 readiness](docs/releases/2.5.1/10-RELEASE-READINESS.md).

## 2.4.3 (candidate)

- Unify URL subscriptions and VLESS keys into one source repository and server pool, preserving source identity, selection and favorites during migration and refresh.
- Coordinate Windows startup, cancellable auto-connect, hidden tray startup and restoration of connection intent; give user-requested restarts fresh launch arguments.
- Reject stale settings saves, keep window visibility out of configuration backup history, and refuse rollback when a required source credential is unavailable.
- Remove only installation-owned user autostart entries during uninstall and discover NSIS without a machine-specific path.
- Confirm process exit during concurrent cleanup before reporting a termination error; make isolated CEF smoke shutdown complete once.
- Highlight only the Updates navigation button and apply the source-map-js 1.2.2 security patch.
- Add installed upgrade, startup, recovery and uninstall acceptance with synthetic encrypted mixed-source data on disposable Windows runners.

Acceptance requirements and limitations: [candidate notes](docs/RELEASE-2.4.3.md).

## 2.4.2

- Reconcile Atlas-TUN/Wintun recovery with current Windows state; preserve working network cleanup and IPC reconnect behavior.
- Import VLESS, TLS/REALITY, XHTTP and other supported transports through the existing subscription pipeline, including plain/Base64 variants and bounded gzip responses.
- Preserve provider transport parameters, server identity, selection and credentials across refresh; skip malformed nodes with diagnostics.
- Add the URL/VLESS source switch and align diagnostic status controls.
- Use transactional upgrade, immutable version directories, original-user activation, stable launcher, authenticated service/UI health and SQLite rollback snapshots.
- Connect native signed download/install handoff and interrupted-update recovery to the existing supervisor.
- Unify local/CI transactional packaging and keep the stable product version at 2.4.2.

Validation evidence and host limitations: [release notes](docs/RELEASE-2.4.2.md).
