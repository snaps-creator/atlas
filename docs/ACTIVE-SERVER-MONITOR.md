# Active server monitoring

## Independent UI reads

The desktop publishes an immutable internal read snapshot separately from its
command mutex. Server lists, connection lists, latency checks, service checks,
pool checks, rules diagnostics and protection checks clone this snapshot without
waiting for connection or settings mutations. No network call holds the read
snapshot lock. The service-status observer performs IPC outside the command
mutex and discards observations from older configuration revisions. An IPC
error is recorded separately and does not count as confirmed core death.

Concurrent frontend proxy-list reads share a single in-flight request. Failed
reads are not cached. Passive pool polling retries after five seconds instead
of waiting five minutes after a local failure. UI errors retain their actual
reason; stale latency is not displayed as a fresh success. Command start/end
events record an operation ID, lock wait and execution time without payloads.

Regression checks hold a command lock while 32 snapshot readers complete, and
exercise 20 simultaneous frontend reads followed by failure and a fresh retry.

The application-owned catalogue checks every node initially and then once per
minute, regardless of the active node's health or the visible page. Failed
nodes use the same cadence. Manual requests and network/profile changes can
trigger earlier checks; batches do not overlap. Successful measurements expire
after 90 seconds; failures replace prior successes immediately. The service
does not run an additional continuous reserve/quarantine sweep.

AUTO and FAILOVER monitor the selected node every ten seconds, with one pending
probe at a time. With autoOptimize enabled, the first sample above
autoSearchPingMs (default 150 ms) starts a bounded six-worker candidate search.
Confirmed failure starts recovery regardless of autoOptimize. A candidate must meet that
threshold. Only the service changes the selector; a fresh node check, real
HTTPS traffic through its live proxy listener and selector confirmation precede
success. Manual selections invalidate automatic work. A failed switch restores
the former selector without claiming that the former path is healthy.

Display and search use the restored 3.0.3 HTTP warm-request metric, with the
existing HTTP 204 and concrete-outbound checks. Authenticated HTTPS route and
tunnel verification remains separate. See [the measured comparison](ping-3.0.3-comparison.md).

An exhausted search requests one bounded subscription refresh and rescan.
Failed incidents use exponential backoff, a refresh cooldown, and a single
Windows notification. A new ticket identifies each permitted refresh within
an incident. The recoveryPolicy defaults preserve old settings compatibility.

See [the October audit, validation and limitations](RECOVERY-REFACTOR-2026-10.md)
for the precise state machine, metrics, tests and the distinction between proxy
listener verification and an end-to-end Windows TUN acceptance check.

Policy-only settings updates change service scheduling without reloading the
network configuration or recreating TUN. Cloudflare latency is not a throughput
measurement and cannot prove that every video destination is working.

## Cleanup regression

Installation cleanup now waits for Atlas-TUN to disappear after service stop.
Uninstall waits for the service and checks its removal result before deleting
the service executable. Post-install also invokes the new cleanup implementation,
because older installers could accept cleanup without a TUN postcondition.
Unknown interfaces are never deleted or reset by alias alone.

The reported incident is present in local history across several launches,
including 2.0.1-alpha.38.1. At inspection time Atlas-TUN and Atlas processes were
absent and AtlasNetworkService was stopped. The old log does not retain the
interface owner's process identity; its exact historical owner is unproven.
This guard prevents silently accepting incomplete cleanup; it does not claim
to repair an unidentified legacy interface automatically.

Validation uses unit tests and isolated loopback processes. No installation,
physical adapter changes, live TUN fault injection, or VM tests are performed.
