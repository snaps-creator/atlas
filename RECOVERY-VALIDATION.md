# Recovery and responsiveness

The route-change/resume reapply policy below describes Beta 15.1. The subsequent
source review in `VPN-CLIENT-RESEARCH.md` replaces it with Mihomo's existing native
network monitor plus WFP-only rebinding on a changed TUN identity. The elapsed-time
resume heuristic and automatic configuration reload on route changes are removed.

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
  health checks trigger session cleanup. Physical route changes are handled by
  Mihomo's interface monitor; they no longer trigger a second full reload in
  Atlas. A changed TUN LUID rebinds the guard only. Missing TUN fails health checks.
- The desktop retries failed connections with bounded exponential backoff, up to
  60 seconds between attempts. Disconnect/Exit cancel recovery intent before
  waiting for application state. A startup delay does not suspend monitoring.
- Tray Exit starts graceful cleanup with an eight-second process-exit deadline.
  An independent service watchdog gives cleanup five seconds after parent exit
  or SCM stop, even if the service command loop is blocked. Closing the service
  releases its dynamic WFP session and its core's kill-on-close job.

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
Corporate split DNS, globally addressed IPv6 LAN prefixes and multi-day stability remain subject
to the office validation plan. Tests do not prove that all possible errors have
been eliminated. These changes have not been installed on the running machine.

See OFFICE-NETWORK-REVIEW.md for the Amnezia/Hiddify comparison, IPv4 LAN route
exclusions, bounded pipe writes, error classification and the isolated three-client
VLESS test. On 2026-09-22, 55 Rust tests and 18 frontend tests passed; frontend
production build and native Windows release build also completed. These checks
do not replace the live acceptance scenarios above.

The service now requests a bounded TUN disable before killing its core and
flushes the resolver cache after cleanup. A separate internal observer survives
individual service termination and performs DNS-cache cleanup afterward. Tests
replace the actual DNS flush with a no-op: no live resolver state is changed.
Killing the observer as well defeats that fallback; third-party application
DNS caches are outside the Windows resolver cache and are not manipulated.

Tray Exit regression checks (2026-09-22): shutdown uses dedicated native threads,
not the shared async blocking pool. The independent eight-second desktop exit
deadline stays armed until process termination, including when posting the UI
exit event stalls. Pending tray commands check the shutdown flag after acquiring
the application lock; they cannot reconnect after Exit. Close-to-tray is disabled
during shutdown. Tests cover a held application mutex and actual termination of
an isolated, deliberately stalled subprocess. The service's separate five-second
parent-death deadline and DNS observer cleanup can outlive the desktop briefly;
eight seconds is not a deadline for every process in the entire session.
The installed application's tray and WebView child cleanup have not been tested
live; these checks do not establish end-to-end shutdown on the affected office PC.

An intermediate run had 58 passes and one failure in the VLESS fixture. Its
origin served concurrent clients serially while draining each connection's
half-close. The origin now handles connections concurrently; no retry was added
to hide request failures. Six consecutive three-client recovery runs passed,
followed by two complete 61-test runs. This is a test-server correction, not a
claim that it caused the office outage.

The final process-chain test uses four real, isolated Windows test processes:
desktop, deliberately blocked service, job-owned core and cleanup observer.
It exercises the production shutdown deadline, service parent watcher, Windows
Job termination and observer callback; all four process handles signal exit and
the observer's cleanup callback runs. DNS flushing is still a test no-op; SCM,
the actual WebView and live office TUN are not exercised by this fixture.
Final regression log: temp/exit-recovery-regression.log (61 passed, zero failed).

HTTP-forwarder coverage was restored alongside SOCKS5 after correcting the
origin's serial connection handling. Three consecutive combined-protocol runs
passed, without retries masking individual request failures. Deferred frontend
commands also recheck shutdown immediately before dispatch, and a request arriving
during Exit cannot leave reconnect intent enabled. Final checks including these
changes are recorded in temp/exit-recovery-final.log.

LAN policy update: TUN and the Atlas WFP guard now share one set of local
IPv4/IPv6 scopes. It includes RFC1918, IPv4 link-local and local multicast,
IPv6 ULA/link-local and link-local multicast. Limited DHCP broadcast is excluded
from TUN but allowed by the guard only through the existing UDP DHCP rule.
Public unicast, fake-IP IPv4 and global IPv6 multicast are not added to the
bypass. The list is independent of Rule/Global/Direct mode. Native Windows
firewall policy is not disabled or overridden with hard-permit filters.

The VLESS fixture now runs eight independent cores with identical credentials
against one loopback server, using both HTTP proxy and SOCKS5. All clients
observe the server outage concurrently, reconnect after its restart, and keep
working when one client stops. This exercises real cores without TUN/WFP on the
host. It does not simulate DHCP lease expiry or eight physical office PCs.
Final logs: temp/lan-policy-final.log and temp/lan-policy-build.log.

### Four-report follow-up (2026-09-22)

The four startup reports show VPN dial timeouts followed by DIRECT DNS failures.
The generated `direct-nameserver` incorrectly inherited `#ATLAS`; it now uses
`#DIRECT` and does not follow proxy DNS policy. The proxy/default resolvers remain
separate. A real bundled-core regression uses a closed VPN port, local UDP DNS
and a local HTTP origin: a domain-based DIRECT request must return HTTP 200.
Only resolver addresses and listener ports are substituted; the generated DNS
proxy suffixes are retained. TUN and OS networking are not enabled in this test.

The top-level IPv6 option now follows the user setting instead of being forced
on. This prevents IPv6 outbound resolution/dialing when IPv6 is disabled. The
Windows core's strict-route IPv6 blocking remains in force when no IPv6 TUN
address is configured; this is not a promise of IPv6 LAN/DHCPv6 connectivity in
IPv6-disabled mode. IPv4 DHCP exclusions/UDP 68-to-67 permission remain intact.

Service health checks now use one bounded background probe rather than waiting
on HTTP inside the command loop. GET requests for statistics, connections and
diagnostics use bounded background jobs as delay probes already did. The command
loop can handle stop/select while those reads wait. The local HTTP client reuses
connections, has a one-second connect deadline and a three-second ordinary request
deadline (12 seconds for URL tests). At most 16 outstanding query jobs are retained;
they expire after 30 seconds. Pending health results are discarded on reconfigure.
Three failed local core health checks still terminate the broken session so the
existing recovery controller can restart it. A remote VPN failure alone does not
restart TUN. AUTO/FAILOVER recovery remains the core's responsibility and manual
server selection remains manual.

The main page now describes process readiness as “Туннель запущен” and displays
the existing real protection-check detail rather than implying that a live local
controller proves internet availability. No new periodic network scan was added.

Validation: 69 Rust tests and 18 frontend tests passed; frontend production build
passed. This includes eight concurrent clients sharing one VLESS identity, server
restart recovery, the isolated process-exit chain, and the new DNS regression.
These reports were captured after startup, not during the office outage. They do
not establish its sole cause, and this run does not establish physical DHCP lease
renewal, sleep/resume, interface switching, or multi-day office stability.

### Sequential hardening follow-up

1. UI snapshots now read a separate published-state store. The network mutation
   lock still serializes changes, but a blocked start/recovery does not prevent
   reading state. Connecting is published before the blocking operation; the
   recovery worker publishes completion/error and refreshes state every two seconds.
2. Every connection attempt has its own cancellation token. Disconnect/Exit cancel
   it without acquiring the network lock. Validation, service launch and pipe
   reads/writes observe cancellation. Closing the cancelled pipe also cancels
   service startup via an independent pipe watcher. The UI exposes Cancel during
   Connecting. Disconnect still awaits cleanup, including for updater callers.
   A real named-pipe test stalls the start reply, cancels it, and verifies that
   the service watcher receives cancellation within a two-second test deadline.
3. Reconfiguration captures the live selector and old configuration bytes before
   applying changes. Runtime or disk-commit failure restores both, checks the core,
   TUN enabled flag and selector, and restores the files. An unverified rollback
   stops the session. The real-core test deliberately makes previous.yaml a
   directory to fail the commit after application; it verifies restoration of a
   manual selector that differs from the old YAML default. A second injected disk
   failure prevents rollback and verifies that the core is stopped.
4. Published revisions change on save/connect/disconnect. Latency results and
   queued batch work belong to a connection/configuration epoch. A late result or
   finally block from an earlier epoch cannot overwrite/unlock the new test.
5. URL tests and controller reads have separate admission limits of eight each.
   Stop/select use neither quota; local health has its own single probe. A stalled
   URL-test saturation test verifies that state queries still complete. Controller
   requests retain their existing deadlines; this does not spawn unbounded workers.

DHCP regression inspects the exact conditions used to install Atlas WFP permits:
UDP 68-to-67 and 546-to-547, without a destination condition, covering broadcast
discovery and unicast renewal in Atlas's own policy. This does not exercise the
Windows DHCP client, other filters, or actual lease renewal. Existing tests cover
TUN identity Keep/Rebind/Missing decisions, not live sleep or Ethernet/Wi-Fi changes.
No production adapter, installed client or network setting was modified.

An intermediate full run had 73 passes and one failure in the eight-client
recovery fixture: one SOCKS request after server restart returned no valid HTTP
status. The core log showed prior outage refusals; the restarted server log showed
successful DIRECT routing. This is not enough to assign the failure to production
recovery or the fixture. Inspection also found generated background health URLs
were public in test cores. Test-only configuration now substitutes a loopback URL
for every group's health URL; production configuration is unchanged. Three
consecutive eight-client runs then passed with no per-request retry added.
The isolation defect is corrected; the isolated failure's cause is unconfirmed.
Evidence: temp/sequential-fixes-final.log and temp/multi-repeat-{1,2,3}.log.

Final verification: temp/sequential-verified-tests.log has 74 Rust passes, zero
failures; 19 frontend tests passed and the frontend production build completed.
The Windows release build completed (temp/sequential-release-build.log). The
installed executable was not replaced and these changes have not been released.

### Bounded owned-process termination

Core stop/drop no longer use an unbounded Child::wait after ignoring kill errors.
They request termination, release KILL_ON_JOB_CLOSE before waiting, and confirm
exit on the owned process handle with a two-second deadline. Unconfirmed exit
returns an error including the kill failure and retains the child in Core::stop
for a subsequent cleanup attempt. Drop also uses bounded cleanup and records any
failure. The deadline is for process-exit confirmation, not the entire TUN/API/IPC
shutdown chain. Configuration validation cancellation/timeout uses the same bounded
exit helper. This change does not make spawn/Job assignment atomic.

The full suite passed 76 tests before the additional fallback-process fixture;
all three termination-helper tests then passed, including a real isolated process
terminated through Job close with kill failure injected. No installed process was
stopped. Logs: temp/stop-fix.log and temp/stop-fallback.log.
