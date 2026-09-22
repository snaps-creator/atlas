# Incident TXT, schema 2

The Diagnostics → Download TXT action now captures a failure investigation bundle.
It does not disconnect VPN, select a node, refresh subscriptions, enable auditing,
change routes/DNS, or alter filtering policy. Node tests update Mihomo health history;
normal AUTO recovery can still occur independently. Capture during the failure,
before closing Atlas or rebooting. Beta 17.2 includes a matching signed installer.

## Evidence collected

- Request-time status, selection, configuration revision, application logs, public
  endpoint/transport fields, subscription update times, configuration fingerprints.
  The final cached revision/configuration is included to identify changes mid-test.
- A passive flight recorder across app restarts: status, selection, core health
  histories, recent core errors and periods when the app lock could not be read.
  It samples roughly every 15 seconds plus bounded API duration. It makes **no
  internet probes**. It retains at most 240 entries / 2 MiB and appends only new
  redacted records to `incident-history.ndjson` in the application data directory.
  Rotation and expiry mean it is not an unlimited or packet-level history.
- Before/after core version, selected groups, all nodes' health history and per-URL
  results, timestamped core errors. Per-URL IDs survive URL redaction.
- Four parallel HEAD requests: two fixed control hosts, each via the local mixed
  proxy and the ordinary Windows path. Records duration, HTTP status, timeout,
  connection errors and nested error chain. The Windows path is subject to active
  TUN/WFP and routing rules; it is **not a physical VPN bypass**.
- A bounded UDP DNS query to Atlas's local DNS listener: response code, answer count,
  truncation and elapsed time. Fake-IP replies do not prove upstream DNS works.
- Up to six actual proxy nodes: selected node first, then failed/healthy examples,
  tested against both control URLs with five-second core probe timeouts. DIRECT,
  REJECT and groups are excluded. No group selection-reset endpoint is used. This
  is explicitly a sample, not a fresh assertion about every node in the pool.
- Adapters, MTU/metrics, addresses and DHCP lease times, DNS, routes, neighbor state,
  error/discard counters, system proxy, DNS socket owners, core TCP states/source IPs,
  bounded route lookups, boot time and resource pressure.
- An asynchronous service-side privileged snapshot: running binary paths, versions
  and SHA-256, WFP collection status, existing WFP events, Security 5152/5157 drops,
  filter details associated with Atlas/Mihomo or recorded drop IDs, firewall profile
  state, recent relevant Windows system events. No audit policy is enabled or reset.
  Event queries are capped (150 Security drops, 300 System events) and can miss older
  events in a busy system. Empty/disabled logs are not evidence of no filtering.

## Bounded execution and availability

The main app lock is not held while collecting. A separate cached client/configuration
permits export when that lock is busy; its age and revision are shown explicitly.
Windows collection is limited to 25 seconds, privileged collection to 24 seconds,
service result polling to 28 seconds, HTTP probes to 6 seconds and DNS to 3 seconds.
Export waits up to 85 seconds after file selection and writes partial results with
missing sections. In-flight bounded requests can finish after that deadline; no new
node test is started after it. Concurrent export/WFP collection is rejected.

An old or stopped service cannot provide the new privileged snapshot. Its explicit
error remains in the report; ordinary Windows evidence/history is still exported.
The recorder only accumulates history after this source version is run. Short events
between samples and events before installation cannot be reconstructed retroactively.

## Rules for interpreting evidence

| Evidence | Supported conclusion |
| --- | --- |
| Local controller/IPC error | A local control request failed; inspect exact error, session revision and process/service data |
| Proxy control request succeeds while Windows-path request fails | Paths differ; correlate rules, DNS, TUN and WFP before assigning a component |
| Successful individual proxy URL test | That node/path worked for that control at that timestamp; no universal server outage then |
| WFP drop with matching process, address/time and filter ID | Windows filtering rejected this flow; use the matching filter's action/owner/conditions |
| `read tcp` error | A TCP socket existed; not proof of successful TLS/Reality/authentication |
| Explicit TLS/certificate/authentication error | The named protocol stage failed; preserve the exact message and clock/configuration evidence |
| Generic Mihomo `connect error: context deadline exceeded` | May include TCP dial **or** TLS/Reality handshake; do not label it TCP SYN failure |
| All sampled URL tests time out | These tested paths did not answer; not proof the servers are globally down |

A client-only TXT cannot identify a silent drop outside the machine versus a silent
server failure with universal certainty. That distinction requires matching server-side
telemetry or a contemporaneous independent check with matching endpoint/transport.
The report supplies concrete evidence and marks missing evidence rather than inventing
a binary provider/Atlas verdict. No 100% root-cause guarantee is claimed.

## Sources and validation

- [Microsoft: netsh WFP](https://learn.microsoft.com/en-us/windows-server/administration/windows-commands/netsh-wfp)
- [Mihomo controller API](https://wiki.metacubex.one/ru/api/)
- Live read-only Windows verification: parser handled multiple XML roots emitted by
  `netsh wfp show state`; 1188 filters scanned, 79 matching filters extracted.
- Automated checks cover bounded history, secret redaction, conservative conclusions,
  HTTP failure versus connection timeout, excluding DIRECT from VPN evidence,
  collector timeout/partial output and section failure isolation.
