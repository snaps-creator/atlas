# Recovery and responsiveness

Implemented changes:

- A selector-only save uses PUT /proxies/ATLAS, without validation subprocess,
  configuration reload, TUN recreation or WFP reinstallation. UI-only saves also
  avoid core reload. Storage failure attempts to restore the previous selector.
- Server latency probes run as bounded background jobs in the broker (16 maximum,
  12-second HTTP deadline, abandoned results expire after 30 seconds). They no
  longer occupy the control pipe while waiting for an external server.
- Statistics, public IP and application enumeration no longer hold the global
  application mutex during expensive work. Snapshot refresh cannot overlap itself;
  busy refreshes preserve the previous UI state instead of accumulating requests.
- Broken/timed-out IPC sessions are invalidated so late responses cannot be used
  as responses to subsequent commands. Busy IPC is not classified as core death.
- The service checks local core health every five seconds. Three failed local
  HTTP checks trigger session cleanup. Default IPv4/IPv6 route identity changes,
  TUN LUID changes and a monitoring gap over 30 seconds trigger configuration
  reapplication and WFP rebinding. The monitoring gap is a resume heuristic.
- The desktop retries failed connections with bounded exponential backoff, up to
  60 seconds between attempts. Disconnect/Exit cancel recovery intent before
  waiting for application state. A startup delay does not suspend monitoring.
- Tray Exit starts graceful cleanup with an eight-second fallback exit. The
  service's existing parent-process monitor and dynamic WFP session own cleanup.

Automated verification uses an isolated Mihomo process with random localhost
ports and TUN disabled. It verifies selector changes preserve process identity
and configuration bytes, invalid selections fail, and a killed test core can
start again. No installed VPN, system proxy or routing table is changed.

Required live acceptance (not performed on the user's active network):

1. Two office PCs for 48–72 hours, including multiple natural DHCP renewals.
2. Ethernet/Wi-Fi transitions and removal/reappearance of the default gateway.
3. Sleep/resume and TUN adapter recreation: verify route recovery and WFP LUID.
4. Kill/hang only the test installation's core; verify recovery, cancellation,
   no duplicate cores and eventual removal of owned filters on Exit.
5. Switch servers during latency tests and invoke Exit during an outstanding
   operation. Verify UI responsiveness and service cleanup after fallback exit.
6. Remote VPN outage: manual selection is preserved; AUTO/FAILOVER remain governed
   by Mihomo's configured health checks. Local controller health is NOT proof
   that the remote VPN server is reachable. Test both types of failure separately.

Limitations: active TCP sessions may break when their network path changes;
applications must reconnect. Recovery currently retains the existing policy of
releasing the failed core's dynamic guard before starting a new session; this
does not establish uninterrupted fail-closed protection between sessions.
Corporate split DNS, IPv6 LAN exemptions and multi-day stability remain subject
to the office validation plan. Tests do not prove that all possible errors have
been eliminated. These changes have not been installed on the running machine.
