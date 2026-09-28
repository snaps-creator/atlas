# Atlas Alpha 2.1.1

## Subscriptions
All saved subscriptions refresh on application startup. Downloads run outside the application mutation lock. Automatic connection waits until refresh attempts finish. Failed downloads preserve the previous nodes. Deleted subscriptions and newer manually refreshed nodes cannot be overwritten by late startup results. Errors remain visible on the subscription and in the incident history. This release does not schedule periodic subscription downloads.

## Concurrency
Read snapshots for server lists, checks and diagnostic requests no longer compete for the mutation mutex. Mutating commands wait instead of returning Atlas busy. Service status is queried outside that mutex; IPC errors are distinct from confirmed process/session termination. Concurrent frontend proxy reads share only the in-flight request. UI retains the known server while explicitly marking a failed status read. Recovery retries local check failures after five seconds.

## Diagnostic evidence
CI embeds the source commit in ATLAS_BUILD_ID so incident history identifies the actual installed source revision. Report schema 4 covers Windows adapters, routes, DNS, proxy and services; privileged WFP evidence; selected core route, traffic and logs; DNS and HTTP probes; operation errors and subscription refreshes. Each exported collector has an explicit coverage state. Captured evidence is not automatically a healthy verdict.

Failed service operations carry session/epoch, recent core logs and the last pre-cleanup failure snapshot over authenticated IPC. Route-confirmation failures preserve individual control checks and per-URL health. Failed UI commands are flushed to the incident history. Frontend unhandled errors and updater check/download/install milestones are recorded through a bounded diagnostic endpoint; IPC reporting failures cannot recurse. Incident-file writes are serialized across background and error collectors. All request failures include action and elapsed time; mutation commands also record queue/execution time and revision. Controller transport errors retain nested OS/TLS errors; HTTP failures retain status and bounded detail. Subscription download errors distinguish connect and timeout signals.

HTTPS diagnostic probes now use the requested proxy for CONNECT as well as HTTP. Secret redaction runs on JSON string leaves before serialization, retaining parseable nested evidence. No diagnostic collector changes selectors, routes, adapters or firewall configuration.

## Evidence limits
The secondary control now uses HTTPS HEAD on cp.cloudflare.com/generate_204 and requires exactly HTTP 204, matching the primary HTTP control. These are two transports to the same Cloudflare service, not independent providers. An HTTP error response is preserved separately from failure to connect. The former trace URL returned 404 for HEAD and is no longer used.

Windows NCSI domains use real DNS answers instead of fake IPv4 addresses. This prevents synthetic IPv4 connections to IPv6-only probe names without enabling IPv6 or changing routing policy. DNS evidence distinguishes CNAME-only replies from replies containing an A record. Native Windows collector output is decoded as OEM before the report is emitted as UTF-8.

Shutdown records and flushes each stage before and after execution: owned core, service, proxy restoration, legacy filters and TUN release. Core evidence includes process ID, initial/remaining TUN LUID, graceful close timing/error and confirmed process exit. Successful Stop replies also return cleanup evidence before the service exits. This improves localization of a stall; it does not prove a Windows driver-level cause by itself.

A silent remote timeout cannot uniquely identify filtering versus peer failure without external evidence. Installer failures before application startup need installer logs. A process killed before delivering IPC evidence can leave a gap. Windows snapshots describe capture time, not an unrecorded past state. Missing evidence is reported; no claim of universal fault identification is made.

## Safe regression checks
Rust suite covers controller IPC contention/cancellation, recovery, loopback proxy failures, cleanup ownership, DNS classification, secret redaction, missing collectors and stale subscription refreshes. Frontend suite covers concurrent reads and existing routing/UI behavior. No VM, live VPN fault injection or changes to the host network are used.
