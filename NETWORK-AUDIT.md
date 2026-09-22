# Network audit, 2026-09-22

## Scope and evidence

Reviewed the connection path through subscription parsing, generated Mihomo config,
service IPC, TUN/WFP policy, kernel health/restart logic, URL tests, frontend queues,
selected-node display and support export. This is not a proof that every function
in the repository is correct or a reproduction on the failing office PCs.

Available original reports: 1790083643, 1790088918, 1790089292. Older report conclusions
exist in conversation history, but the original files are no longer present in Downloads.
Do not treat historical summaries as newly revalidated evidence.

| PC/report | Confirmed observations | What this does not establish |
|---|---|---|
| .73 / 1790083643 | Beta 16; connection/handshake failures to 82.27.34.108; earlier DHCP failure followed by a valid lease and a new core | Which component failed or discarded packets |
| .82 / 1790088918 | Beta 17; 62 nodes, 4 alive and 58 false in core history; failures to 87.85.136.231 and 84.201.132.141; valid DHCP lease | Global server outage or identity with another device's cached configuration |
| .73 / 1790089292 | Beta 16; 238 connection/handshake timeouts to 82.21.125.151 in roughly two minutes, including 7 explicit dial errors; subscription updated during incident; valid lease | Whether selection was manual or AUTO (Beta 16 export omits this) |

The installed local application was verified as Beta 17. Read-only controller
inspection found 124 nodes (106 alive, 18 failed at sampling). The saved Clash
configuration has 62 nodes and TUN disabled. Both configurations use gVisor,
MTU 1500, strict-route and auto-detect-interface. Atlas has LAN route exclusions.
Clash enables tcp-concurrent; Atlas does not. All sampled server addresses are
literal IPv4, so DNS address racing is not an explanation for their TCP timeouts.
These samples are not a controlled same-PC, same-endpoint failure comparison.

Two Atlas subscriptions contain different endpoints under the same location name:
Amsterdam 82.21.125.97 and 82.21.125.67. The saved Clash config uses .97.
Matching labels alone cannot establish identical endpoint/transport configuration.

## Confirmed defects changed in this branch

- Manual all-node checks used three workers and four sequential eight-second attempts
  per node. Replaced with a single native Mihomo group request with a five-second
  deadline, terminal results for all members, and an 18-second UI IPC deadline.
- Cards discarded core history, leaving missing UI results despite completed core
  tests. Cards now read existing history; no network test is launched by that read.
- The selected-node widget launched its own multi-attempt tests every 30 seconds.
  Removed those tests; it reads core history and reports selected-node/control failure.
- Repeated traffic errors could restart whole-pool tests as soon as the previous
  test ended. Added a 30-second start-to-start cooldown; first trigger is immediate.
- Reports omitted endpoint identity, subscription update times and core TCP sockets.
  Export now includes a whitelisted endpoint inventory, TCP socket states and bounded
  Windows route lookups for peers. Credentials and subscription URLs remain excluded.

## Diagnostic meaning

Important correction to earlier analysis: in Mihomo VLESS DialContext the same
`connect error` wrapper is used for dialContext and StreamConnContext. A plain
`context deadline exceeded` does NOT identify TCP SYN failure versus TLS/Reality
handshake failure. `read tcp` proves an established socket; these cases must not
be collapsed into one TCP-dial explanation. The export records separate counts.

Local controller unavailable is evidence of a local control failure, not proof of
remote server failure. A successful proxy URL test proves that this tested path
worked at that time; it does not prove every connection or application works.
Timeouts from all nodes prove that this PC obtained no successful probe response.
They cannot alone distinguish a local filter, upstream path failure, protocol
configuration failure and remote endpoint failure. A successful independent device
check rules out a simultaneous universal outage, but does not identify which local
component failed. Never label these timeouts as confirmed provider outage.

Connected currently describes a running local session, not guaranteed remote
reachability. Kernel liveness checks only query /version. The selected-node and
pool indicators must be read separately; a full session-state redesign is pending.

## Unresolved work / release gate

The cause of the observed office connection failures has NOT been reproduced or isolated.
No single root cause, full fleet reliability, or complete repair may be claimed.
Need a failing-PC snapshot with endpoint inventory and socket/route evidence and,
where ambiguity remains, a controlled comparison using the identical endpoint and
transport with Atlas and another client. Packet/filter evidence is needed to assign
responsibility for silent drops; a timeout cannot supply that evidence.

Group checks and recovery are not yet coalesced across all callers. Pool recovery
still lives in the desktop webview. Subscription recovery depends on the local
proxy. LAN name resolution under strict-route still needs a printer-host test.
The 17.1 manual NSIS installer was rebuilt successfully after these changes.
It has not been installed over the user's running Beta 17 or published to GitHub.

The old DNS fixture still failed once after the earlier responder-loop repair.
Its HTTP origin also closed after one TCP read, incorrectly treating that read as
a complete HTTP header. It now consumes complete headers and drains the close,
as the other multi-client fixture does. Core output is retained on failure.
This fixes a definite fixture defect; it does not identify an office outage cause.

## Live follow-up after user restarted both clients

On 2026-09-22 the user stopped Atlas, successfully used Clash, closed its window
and started Atlas. Atlas core initially reported 122/124 alive; a later snapshot
reported 108/124. These are changing probe histories, not a controlled comparison.
Clash service/core remained, listening on TCP/UDP 53 and proxy 7897. Atlas used
11053/17890/19090 and its own active TUN. Windows system proxy was disabled.
No persistent Windows block was reproduced by this sequence.

Same-endpoint Amsterdam nodes in saved Clash and Atlas shared address, port and
UUID but differed in SNI and Reality short-id. A provider can issue different
parameters; this comparison alone does not show which is valid or why others fail.
Explicit Clash packet-encoding xudp versus omitted Atlas value is equivalent in
the inspected Mihomo VLESS default and is not itself a defect.

Confirmed URI import defect: fingerprint was read only for Reality; ordinary TLS
lost explicit fp, and ALPN was omitted. Both are now preserved when TLS is enabled.
Existing imported nodes require subscription refresh to pick up this parser fix.
No certificate verification bypass was added.

Selected-node UI now reads health for the selected group's test URL, matching
AUTO's per-URL decision. Diagnostics preserve per-URL results with stable URL IDs
that survive URL redaction, plus DNS listener owners and Windows proxy state.

## Sources inspected

- Mihomo adapter/adapter.go: URLTest and per-URL health state.
- Mihomo adapter/outboundgroup/groupbase.go and hub/route/groups.go: concurrent
  member tests, one context deadline, omitted failed members and all-failed HTTP 504.
- Clash Verge src/services/delay.ts: bounded concurrent tests and separate UI state.
- https://sing-box.sagernet.org/configuration/route/ : interface binding and loops.
- https://sing-box.sagernet.org/configuration/inbound/tun/ : strict Windows DNS policy.
- https://wiki.metacubex.one/ru/config/inbound/tun/ : routing exclusions and strict-route.

## Validation

Real bundled Mihomo fixture: 62 loopback nodes, 3 healthy / 59 accepting TCP but
stalling SOCKS handshake: 5.03 seconds; all 62 receive terminal results. A second
fixture checks 62 failed nodes. Neither fixture installs TUN or changes host routes.
These fixtures verify scheduling/timeout behavior, not the office outage's cause.

Final source checks after incident-export follow-up: 90 Rust tests, 26 frontend tests, frontend
production build passed. Live Atlas core sockets use physical source 192.168.1.83;
established connections to its VPN endpoint were present. No installed binaries,
routes, adapter state or other client's processes were changed during this audit.
