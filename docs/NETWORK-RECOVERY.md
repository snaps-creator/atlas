# Network recovery investigation — 2026-10-07

## Source identity

The initial workspace was 2.2.1-alpha.1 (`0bb6774`). The two reports identify
2.3.1-alpha.55.1; the later history records build
`79d08649e7413e7ff10ddff8f8aceb1f8261205e`. This commit was fetched and is the
base of this change. The release workflow stamps its run number onto the base
product version. The initial source-version limitation is therefore resolved.

The older description-string test is **not** the diagnosis of 2.3.1. The report
build verifies registry driver identity and additionally caches a positive
LUID/GUID result separately in each process. This permits different results
after Windows withdraws registry entries. The reports do not establish whether
this race, differing registry access, or another timing difference caused each
individual unsuccessful query. They do establish a stale CleanupError after a
later reusable/down observation.

## Confirmed implementation defects and changes

1. `App::connect` rejected CleanupError without inspecting anything. Retry now
   runs reconciliation, preserves the old failure in history and updates the
   current verdict from fresh observations. Cleanup completion requires stopped
   SCM state, released owned processes, no remaining TUN routes and successful
   owned-proxy/filter restoration. Early stop errors remain diagnostic evidence
   even when their postconditions subsequently converge.
2. Driver recognition depended on process-local history. `network_guard` now
   classifies current evidence as Missing, Available, DisabledReusable,
   CleanupRequired, InvalidDriver, ActiveForeign, ActiveOwnedByAtlas or Unknown.
   Ownership requires explicit authority; unowned Wintun activity is Unknown.
   Available means Windows reports an inactive interface and no routes, with
   unavailable driver evidence. It does **not** mean verified Wintun and does
   not authorize deleting that adapter. Route deletion still requires an exact
   Atlas LUID and current down/verified driver evidence (or an absent stale
   interface); the evidence is checked again before each deletion.
3. A lost pipe ended the on-demand service unconditionally. A committed session
   now has a bounded reconnect window; the authenticated desktop process
   observer continues to own its lifetime. The client queries SCM before
   reconnecting and verifies the pipe belongs to the same PID. No mutation is
   replayed after an uncertain reply. Stopped, starting and unknown SCM states
   are distinct from channel loss. A nonzero exit code is reported as evidence,
   not automatically called a crash.
4. Service health verification happened after Core had committed the candidate
   as last-working. There were two nested rollback mechanisms, one regenerating
   the old configuration. The replacement is preflight in a private loopback
   core, then one apply/verify/commit transaction. The old YAML, live selector
   and Xray workers survive until commit. Rollback restores those exact bytes
   and verifies the restored path. A failed rollback stops the owned core and
   the service abandons its session through its existing bounded cleanup path.
5. URL validation returned the same text for unrelated conditions. Typed error
   codes and distinct Russian messages now cover the requested cases. Logs
   contain the code only. The masked Paper screenshot cannot identify which
   URL condition actually applied to that user's input.

## Recovery evidence and boundaries

The privileged service records its verified LUID/GUID identity as one value in
HKLM\SOFTWARE\AtlasVPN\NetworkOwnership. Desktop and maintenance readers can
use this same instance evidence if Windows withdraws the driver class key;
an explicitly conflicting driver still overrides it. Deleting residual routes
requires this ownership record and a matching current GUID, not just the alias
or display name. Legacy residue without provable ownership remains a reported
failure instead of authorizing deletion of potentially foreign routes.

Cleanup stages share an operationId. Candidate transactions have their own
operationId and PreparingCandidate, RollingBack, PreviousRestored,
RecoveryRequired and Committed events. The baseline event reports current SCM,
adapter/driver/route evidence, owned proxy restoration, historical stop errors,
and ReadyForRetry versus CleanupRequired. Interface DNS configuration is not
rewritten by this change. Existing export-time direct HTTP/HTTPS/DNS probes
remain the source of connectivity evidence; retry also performs a bounded direct
HTTPS probe after successful reconciliation. Local cleanup success alone does
not assert that the provider or the Internet is reachable.

Native alias lookup failure is followed by interface enumeration before
declaring Missing. Microsoft documents nonzero alias-lookup results as failure,
not proof of absence: [ConvertInterfaceAliasToLuid](https://learn.microsoft.com/en-us/windows/win32/api/netioapi/nf-netioapi-convertinterfacealiastoluid),
[GetIfTable2Ex](https://learn.microsoft.com/en-us/windows/win32/api/netioapi/nf-netioapi-getiftable2ex).

## Verification matrix

| Scenario | Verification |
| --- | --- |
| Cleanup failure followed by fresh reusable baseline | recovery reducer and TUN classifier regression tests |
| Repeated cleanup/current no-routes state | TUN classifier, recovery reducer and existing stopped-core tests |
| Live process vs lost channel vs stopped/starting process | channel decision test; installed-service acceptance reconnects a real pipe and compares SCM PID/network epoch |
| Failed candidate before live apply | real Mihomo preflight failure leaves live PID and last-working bytes unchanged |
| Failed candidate health, successful rollback | real Mihomo test verifies selector, PID, old bytes, and both verification phases |
| Candidate/rollback failure | real Mihomo commit/rollback fault test verifies owned core termination and repeatable stop |
| DNS/proxy/route isolation | existing loopback DNS, proxy ownership and foreign-route policy tests; disposable-runner active-TUN upgrade acceptance |
| URL conditions | typed URL regression test, including missing host and secret-free errors |

The loopback tests do not prove every real Windows TUN/WFP/SCM failure sequence.
The disposable-runner test covers active-TUN upgrade/reopen and idle-session IPC
reconnect; it does not inject every possible active VPN rollback/driver crash.
No blanket guarantee for all Windows network failures is claimed. The exact
initial provider timeout cause and a service crash are not established by the
supplied reports. Publishing is gated on the existing unit/integration suite,
the newly built installer's disposable Windows upgrade acceptance, signature,
native helper and UI startup checks.
