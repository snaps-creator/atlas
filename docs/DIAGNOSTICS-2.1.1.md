# Atlas Alpha 2.1.1

## Subscriptions
All saved subscriptions refresh on application startup. Downloads run outside the application mutation lock. Automatic connection waits until refresh attempts finish. Failed downloads preserve the previous nodes. Deleted subscriptions and newer manually refreshed nodes cannot be overwritten by late startup results. Errors remain visible on the subscription and in the incident history. This release does not schedule periodic subscription downloads.

## Concurrency
Read snapshots for server lists, checks and diagnostic requests no longer compete for the mutation mutex. Mutating commands wait instead of returning Atlas busy. Service status is queried outside that mutex; IPC errors are distinct from confirmed process/session termination. Concurrent frontend proxy reads share only the in-flight request. UI retains the known server while explicitly marking a failed status read. Recovery retries local check failures after five seconds.

## Diagnostic evidence
Report schema 4 covers Windows adapters, routes, DNS, proxy and services; privileged WFP evidence; selected core route, traffic and logs; DNS and HTTP probes; operation errors and subscription refreshes. Each exported collector has an explicit coverage state. Captured evidence is not automatically a healthy verdict.

Failed service operations carry session/epoch, recent core logs and the last pre-cleanup failure snapshot over authenticated IPC. Route-confirmation failures preserve individual control checks and per-URL health. Failed UI commands are flushed to the incident history. All request failures include action and elapsed time; mutation commands also record queue/execution time and revision. Controller transport errors retain nested OS/TLS errors; HTTP failures retain status and bounded detail. Subscription download errors distinguish connect and timeout signals.

HTTPS diagnostic probes now use the requested proxy for CONNECT as well as HTTP. Secret redaction runs on JSON string leaves before serialization, retaining parseable nested evidence. No diagnostic collector changes selectors, routes, adapters or firewall configuration.

## Evidence limits
A silent remote timeout cannot uniquely identify filtering versus peer failure without external evidence. Installer failures before application startup need installer logs. A process killed before delivering IPC evidence can leave a gap. Windows snapshots describe capture time, not an unrecorded past state. Missing evidence is reported; no claim of universal fault identification is made.

## Safe regression checks
Rust suite covers controller IPC contention/cancellation, recovery, loopback proxy failures, cleanup ownership, DNS classification, secret redaction, missing collectors and stale subscription refreshes. Frontend suite covers concurrent reads and existing routing/UI behavior. No VM, live VPN fault injection or changes to the host network are used.
