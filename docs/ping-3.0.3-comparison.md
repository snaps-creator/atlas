# Ping regression: 3.0.3 measurement restored

## Cause

The 3.0.3 release state at `a243a40` used
`http://cp.cloudflare.com/generate_204` and `unified-delay: true`.
Commit `55622a7` switched to HTTPS and `unified-delay: false`; this
change is included in installed 3.0.4 (`4f1edb0`). The displayed number
therefore changed from a repeated request on a warmed connection to
connection establishment plus the HTTPS request. The 150 ms search
threshold did not change, so healthy nodes could become search triggers.

The previously installed 3.0.2 (`51c1e16`) and installed 3.0.4 have identical
Mihomo and Xray binary hashes. The running Mihomo reports `v1.19.29`.
This is a measurement-policy change, not evidence of a slower new engine.

## Controlled comparison

Completed 2026-10-15 21:04 UTC, on the user's existing Xray SOCKS bridges.
Eight identical concrete nodes, three rounds per method, three workers.
Method order alternated. Disposable loopback-only Mihomo instances used
the same core binary with either unified-delay enabled or disabled.
They did not enable TUN, DNS, or system proxy, change selectors, reload
the live configuration, or restart the VPN. Live configuration digest
and selected-node hash were unchanged after the experiment.

| Method | Successful HTTP 204 | Median of successful samples |
| --- | --- | --- |
| 3.0.3 HTTP, unified-delay true | 22/24 | 55 ms |
| 3.0.4 HTTPS, unified-delay false | 21/24 | 255 ms |
| HTTPS, unified-delay true | 21/24 | 57 ms |

The remaining requests returned controller HTTP 503/504 and are excluded,
not counted as zero or successful pings. All successful samples were
checked against the corresponding URL's `alive` result and HTTP 204.

Per-node medians (successful samples only):

| Node | HTTP warm | HTTPS cold | HTTPS warm |
| --- | --- | --- | --- |
| Almaty | 56 | 275 | 53 |
| Frankfurt | 53 | 265.5 | 66 |
| Paris | 65.5 | 302 | 62.5 |
| Germany Whitelist2 | 54 | 258 | 64 |
| Netherlands Whitelist2 A | 48 | 221 | 47 |
| Netherlands Whitelist2 B | 47 | 218.5 | 48.5 |
| Stockholm | 71 | 287 | 72 |
| Zurich | 61 | 224.5 | 61 |

This reproduces a substantial difference caused by the method alone,
even without 64 concurrent probes. It does not reconstruct the exact
network conditions at the time of either screenshot, or prove that
concurrency and routing never affect latency.

## Requested behavior

- Restore the exact 3.0.3 HTTP warm-request metric for display and search.
- Retain concrete outbound validation, per-URL HTTP 204 validation, and
  the newer failure/identity/epoch handling. This is not a whole-release rollback.
- Retain separate authenticated HTTPS traffic verification before recovery
  considers the live tunnel healthy. Plain HTTP latency alone is not that proof.
- Application-owned whole-catalogue checks: initially, then 60 seconds
  after each result, regardless of the selected server's health.
- Active node: every 10 seconds, with no overlapping active jobs.
- Failure/high-latency recovery still starts independently of the minute timer;
  existing recovery backoff and switching safety checks remain.
- Remove the duplicate service background reserve/quarantine sweeper.
  The minute catalogue includes failed nodes; recovery can retry nodes after
  their quarantine expires and validates a replacement before switching.
- Manual tests, profile/network invalidation and reconnect can request an
  earlier check. Periodic batches do not overlap.
- Measurement expiry: 90 seconds, allowing the minute refresh to complete;
  failures immediately replace old successes and reads do not renew timestamps.

Mihomo unified-delay retains its original fallback behavior: if the repeated
request fails after a successful first request, it can return the first
request's elapsed time. This is part of the requested 3.0.3 metric, not a pure
ICMP RTT guarantee. Separate tunnel verification is intentionally retained.

## Verification

- Final native suite: 288 passed, 5 opt-in tests ignored, 0 failed.
- Explicit real-time timer acceptance: 65,092 ms, exactly two probes,
  first result at 105 ms, maximum observed measurement age 60,045 ms.
- Frontend: 70 tests passed; TypeScript and Vite production build passed.
- Real-core regression adds a 450 ms first-request delay and a 25 ms
  repeated-request delay: warm measurement stays below 300 ms; cold
  measurement is at least 400 ms. Both must be validated HTTP 204.
- Existing HTTPS certificate rejection, proxy/traffic timeout, recovery,
  concrete Xray outbound and bad-credential integration tests pass.
- Active 10-second cadence, full 126-node minute coverage including failures,
  no overlapping jobs, immediate high-latency/failure recovery, and stale
  evidence rejection have regression coverage.
- One earlier full run hit a controller startup timeout on cycle 42 of the
  50-start lifecycle stress test. No production startup policy was changed
  to hide it; the final complete rerun passed all 50 cycles.
- Disposable measurement cores exited; their private configuration directories
  were removed. The installed VPN was not reloaded, restarted or upgraded.

The implementation is based on current 3.0.4 in the attached
`minute-health-checks` worktree. The original dirty checkout was preserved.
Native test logs are in that worktree's ignored `temp/ping-regression.log`
and `temp/ping-minute-timer.log`.

## Blank status in installed 3.0.4

`latencyLabel` deliberately returns an empty string for absent data, queued,
testing, stale and not-connected states. Starting a new probe clears a prior
successful value; the installed release also expires successes after only
10 seconds. Empty therefore does not mean timeout. Confirmed timeout and
controller errors have distinct labels. This patch aligns expiry with the
minute cadence but does not introduce unsolicited status-label UI changes.
