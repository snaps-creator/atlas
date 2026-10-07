# Atlas 2.4.2 — stabilization and review evidence

## Preserved behavior

The release keeps the existing Controller/NetworkSession and subscription lifecycle. URL and manually pasted VLESS sources use the same persisted server model, candidate validation, health check, commit and rollback. Metadata-only refreshes do not restart a healthy session. Invalid nodes are isolated; credentials and provider extras are redacted from diagnostic exports.

Network recovery continues to reconcile actual OS state, distinguish reusable verified Wintun from foreign/active adapters, and reconnect IPC independently of the service process. Installer cleanup targets verified Atlas-owned process paths and preserves unrelated adapters, DNS and routes.

## Installer and updater

The delivered installer uses immutable version directories, a durable transaction journal, a stable native launcher, original-user desktop activation, authenticated UI/service health, SQLite WAL snapshots and rollback. The launcher now delegates interrupted activation to the same recovery supervisor. UI update handoff downloads and verifies signed schema-2 transactional metadata and the complete installer before UAC or stopping Atlas. The independent download worker runs outside the installation tree so installer quiescence cannot terminate it.

Release and PR packaging now use the same transactional wrapper and receipt verification. Stable 2.4.2 remains 2.4.2 in CI. Production signing keys are supplied explicitly through the existing signing environment; no new signing identity is generated. The publication gate remains fail-closed until its documented evidence is complete.

## Observed installed build

The user installed build `local-20261007-2.4.2-1` and confirmed an active VPN connection. Read-only inspection confirmed:

- Upgrade from 2.4.1 to 2.4.2 reached durable `Committed`.
- The registered service points into the active 2.4.2 version directory.
- Logs show successful disconnect, repeated TUN connection and URL/VLESS source switching.
- Recent passive samples remain `Connected`.
- Individual latency-probe HTTP 504 responses are not treated as service crashes; IPC close events during orderly shutdown have exit code zero.

This installed build predates the final CI/native update-handoff changes in this PR. Its success is not substituted for execution of those changes.

## Validation scope

Local checks cover frontend and package contracts, real loopback Mihomo/Xray traffic, refresh/selection/persistence, selective process cleanup, filesystem transaction fault points, SQLite rollback and authenticated candidate health. GitHub's disposable Windows acceptance covers fresh signed packaging, legacy upgrade with a real Wintun, TUN reuse, clean installation and uninstall. It is explicitly guarded against running on a user's workstation.

The active user's VPN was not stopped. A physical Windows reboot and destructive install/uninstall/rollback of the final PR source were not performed on that host. These are environmental limits, not PASS results. A successful clean CI build and system acceptance must be reviewed before merging/releasing. No GitHub Release or merge is authorized by this PR preparation.
