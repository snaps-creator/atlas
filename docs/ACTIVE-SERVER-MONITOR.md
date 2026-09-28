# Active server monitoring

AUTO and FAILOVER monitor the actual selected node. A single asynchronous probe
starts every 10 seconds when the previous probe has completed. Probes never
overlap with themselves; controller errors are recorded separately from remote
probe failures. The control URL is Cloudflare, without gstatic.

The persistent `autoSearchPingMs` setting defaults to 150 ms (range 1–5000).
Three consecutive readings above this threshold enable a background search,
with at most two candidate probes at once. A replacement must improve latency
by both 30 ms and 20%, pass two observations, and respect a 60-second hold.
A completed unsuccessful search waits 60 seconds before another pass. Healthy
active nodes no longer trigger the old five-minute pool scan.

On a failed active probe, recovery takes priority over optimization and
quarantine diagnosis. Two workers search candidates ranked by core health
history. The first confirmed response is submitted immediately; it does not
wait for the other worker. The service alone commits the selector change.
The previous node is quarantined for a later recheck. If replacements are
exhausted, the original node is checked again so a recovered path is usable.
Configuration generations reject stale results; cancellation stops old searches
after their in-flight probe. Manual node selection does not enable auto-switching.

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
