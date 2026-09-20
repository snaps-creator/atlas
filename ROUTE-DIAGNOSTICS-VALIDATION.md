# Beta 12 validation

## Changes

- TUN startup waits for readiness, records stderr and stops promptly on an explicit
  TUN-listener failure. Atlas uses 198.19.0.1/16 for fake IPs and its own IPv6
  prefix to avoid the observed collision with Mihomo's default 198.18.0.1.
- Diagnostics validate the rule actually matched, not just the default route.
  IPv6 requires a remote HTTPS response; a locally accepted TCP handshake alone
  is not treated as external connectivity.
- Protection status is checked every 30 seconds without overlapping requests.
- Live service status replaces a stale disk marker for the active-guard display.
- Visible version comes from Tauri; installer names include the build version.
- GitHub update checks run at startup and every six hours, with a 15-second
  request timeout. Errors retry on the next interval; finding an update stops
  polling until restart. Installation remains user-triggered.

## Automated verification

- Frontend production build passed; 10 tests passed, including scheduled retry,
  prevention of overlapping requests, stopping after an available update,
  cleanup and React StrictMode initialization.
- Network backend: 39 Rust tests passed on the same backend source in Beta 11.
  The reserved-port integration test ran separately with Atlas disconnected.
- The signed Beta 12 installer is rebuilt from this PR's application source.
  Beta 12 has not been installed as part of PR preparation.

## Installed Beta 11 evidence (same network backend)

- Five non-elevated service handshakes passed; the service stopped afterwards.
- IPv4/TCP, intercepted TEST-NET DNS, UDP STUN source-port/rule correlation,
  blocked IPv6 HTTPS and received Telegram traffic passed manual diagnostics.
- Google STUN matched a proxy domain rule; Cloudflare STUN matched DIRECT.
- Gemini, YouTube and telegram.org returned HTTP 200 without a system proxy.
  ChatGPT returned 403 to curl; observed ChatGPT/Codex app flows received data.
  These are connectivity checks, not full application-feature tests.
- Forced desktop exit stopped the service, removed Atlas-TUN and released its
  TCP ports. Subsequent startup exposed the stale-marker UI issue fixed here.
- A scheduled recheck displayed the green protection shield on screen.

## Remaining limits

- An initial HTTPS diagnostic can fail while a later request succeeds. The exact
  failing stage was not recorded; the shorter check interval does not establish
  a root-cause fix.
- The user reported about two minutes for Clash to recover after switching.
  A later snapshot found no Atlas service, routes, adapter, listeners or cached
  fake IPs, but did not capture the delay. Its cause remains unconfirmed.
- No exhaustive physical-interface packet capture was performed for DNS/IPv6.
- A real GitHub update (detection, signature validation, install and automatic
  restart) still requires publishing Beta 12 and updating installed Beta 11.

Final Beta 12 installer SHA256: 4CAFB41F1DD56992A64AF54200A0E53296B1582283E99508E39BDDFE75FD7702.
The matching updater signature is retained next to the root installer.
