# Changelog

## 2.4.2

- Reconcile Atlas-TUN/Wintun recovery with current Windows state; preserve working network cleanup and IPC reconnect behavior.
- Import VLESS, TLS/REALITY, XHTTP and other supported transports through the existing subscription pipeline, including plain/Base64 variants and bounded gzip responses.
- Preserve provider transport parameters, server identity, selection and credentials across refresh; skip malformed nodes with diagnostics.
- Add the URL/VLESS source switch and align diagnostic status controls.
- Use transactional upgrade, immutable version directories, original-user activation, stable launcher, authenticated service/UI health and SQLite rollback snapshots.
- Connect native signed download/install handoff and interrupted-update recovery to the existing supervisor.
- Unify local/CI transactional packaging and keep the stable product version at 2.4.2.

Validation evidence and host limitations: [release notes](docs/RELEASE-2.4.2.md).
