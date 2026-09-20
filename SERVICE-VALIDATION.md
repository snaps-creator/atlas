# Beta 8: installed service regression

## Cause

The desktop client authenticated the named-pipe server by opening its SYSTEM
process with PROCESS_QUERY_LIMITED_INFORMATION. Windows denied this operation
for an ordinary desktop user, producing «Процесс сетевой службы недоступен».
A service-start-only test did not exercise the failing handshake.

## Change

The client compares the pipe server PID with QueryServiceStatusEx for the
registered AtlasNetworkService. Its service DACL grants users QUERY_STATUS and
START, but not CHANGE_CONFIG. A mismatching or stopped service is rejected.
The privileged service still validates the client executable before accepting
commands. Startup retries cover the previous session stopping concurrently.

`atlas-vpn.exe --check-network-service` tests authentication and a status
request without starting Mihomo, TUN, DNS or WFP. It also verifies that the
desktop client's own PID is rejected as a service PID.

Run `scripts/test-installed-service.ps1` as a non-elevated user after installing
the matching build and disconnecting Atlas. It refuses an elevated test process
or an already active service. Five immediate sessions must succeed, then the
service must stop. Clash does not need to be changed for this test.

## Evidence

- Release Rust tests: 37 passed, including missing/unknown protection results.
- Frontend tests: 6 passed.
- Production NSIS bundle and updater signature built successfully.
- Root installer: 39,899,792 bytes; SHA-256
  `F65E4A58DF7336F6D2E2A0800D453F917F1A29D3258C7334DDE0B3F2E94510C3`.
- Installed service regression: pending.
- Live TUN, traffic routing and disconnect/crash: pending.

Passing the service test alone does not establish successful VPN connectivity.
