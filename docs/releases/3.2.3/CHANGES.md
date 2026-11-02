# Atlas 3.2.3

Base: published `v3.1.4`, commit `f1e41e3cb62935b17a9156de7ad297181ff2512b`.

## Server checks and responsiveness

- Bound display checks to 32 workers. Published 3.1.4 used 64; the intermediate local responsiveness build used eight. This change combines 32 workers with a two-second discovery pass and full ten-second retries after discovery timeouts.
- Do not publish a short-pass timeout as definitive failure. Preserve unexpired successful evidence with its original timestamp during refresh.
- Schedule connected checks in bounded chunks. Prioritize explicit requests, selected/favorite nodes and at most 32 visible cards. Refresh successful results without waiting for an entire catalogue pass. No new server-card status labels.
- Keep both disconnected passes in one disposable unprivileged core. Coalesce frontend reads, batch service result polling and discard superseded work.
- Prioritize disconnect in application/IPC queues. Keep disconnect controls usable during pending work. Serialize optimistic settings changes with revision conflict handling.

## Startup and recovery

- Persist startup-search progress across failed connection attempts, including a service client restart, instead of repeatedly trying the same failed subset. Reset progress when the catalogue changes or a connection succeeds.
- Require three consecutive high-latency samples before optimizing a responding path. Alternate existing control directions, keep bounded recent quality evidence and distribute replacement candidates across source/transport families.
- Confirm replacements twice, then verify the actual selected path. Preserve manual pinning, opt-in optimization, quarantine, cancellation and rollback.
- Prepare internal TLS ClientHello fragmentation fallback variants for compatible simple Xray VLESS/Trojan TCP TLS/REALITY profiles. Try ordinary candidates first, share the existing Xray process and leave subscription data unchanged.

## Site and incident diagnostics

- Measure manually requested site RTT with HTTP/2 PING/ACK: median of three completed replies after warmup. Report missing measurements neutrally. This is distinct from server-card HTTP timing and is not proof of account access or throughput.
- Improve HTTP rule-check interpretation, bounded Gemini regional-text inspection, cancellation and context expiry. Avoid diagnostic headers that triggered an observed HTTP 403 challenge.
- Preserve the last failed session before cleanup, current detailed capture, configuration, subscription attempt/application evidence and explicit history coverage.
- Include Xray connections, bounded numeric endpoint/Windows route observations and a bounded one-MiB transfer through the current rules. These measurements do not identify who blocks a path and are not a physical bypass or a maximum-throughput test.
- Collect and save the completed LAN capture to the Windows Downloads folder. Match capture identity and reject stale, incomplete, timed-out or wrong-peer results.

## LAN management and local updates

- View and edit rule groups/rules and subscriptions on a LAN peer. Support subscription add/replace/rename/refresh/User-Agent/delete operations through normal Atlas validation.
- Require separate write permission on the target, peer identity and revision checks. Do not expose saved subscription credentials. Do not retry an uncertain write automatically. Both peers must support the feature.
- Stage a receipt-verified local candidate without stopping Atlas or its service. Activate transactionally on the next user launch. Permit replacement only of a matching, unactivated deferred candidate. Update installed version metadata after successful activation.
- Synchronize product/package/Cargo/readiness version metadata to 3.2.3.

## Validation and limits

- 85 frontend tests and the production TypeScript/Vite build passed.
- Targeted Rust suites cover scheduler/lifecycle, stale results, probe evidence, IPC batching/cancellation/admission, startup continuation, recovery/transport fallback, LAN operations and deferred updates. A real-timer loopback test confirmed refresh without an open server page over 65 seconds.
- A scaled local-controller fixture compares eight-worker full checks with 16/32-worker phased checks on 96 nodes: 32 unavailable and 64 healthy, including slow healthy nodes requiring a full retry. It verifies complete final classification and bounded concurrency. This is synthetic algorithm evidence, not a user-network speed guarantee.
- The local 3.2.3 installer was extracted and all 32 packaged files/version metadata verified; a corrupted payload was rejected. Source hashes matched the build snapshot. Deferred staging preserved the active application and service processes.
- Signed clean CI builds, installed upgrade/recovery acceptance and publication preparation remain required for this exact PR candidate. Local packaging does not replace those gates. Physical Windows reboot/power loss during upgrade remains untested as disclosed in the existing readiness policy.
