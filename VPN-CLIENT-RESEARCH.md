# Windows VPN client comparison — 2026-09-22

This is a source-level comparison, not a diagnosis of the other office PC.
The affected PC's failure has not been captured. A successful check on the
developer's PC cannot establish the cause or verify a fix on that machine.

## Sources actually inspected

- Clash Verge Rev, commit `ed0b8015738de1afdac2c260f4b7961247127c89`:
  [TUN defaults](https://github.com/clash-verge-rev/clash-verge-rev/blob/ed0b8015738de1afdac2c260f4b7961247127c89/src-tauri/src/config/clash.rs),
  [constants](https://github.com/clash-verge-rev/clash-verge-rev/blob/ed0b8015738de1afdac2c260f4b7961247127c89/src-tauri/src/constants.rs),
  [lifecycle](https://github.com/clash-verge-rev/clash-verge-rev/blob/ed0b8015738de1afdac2c260f4b7961247127c89/src-tauri/src/core/manager/lifecycle.rs).
- Mihomo **v1.19.31**, the version reported by Atlas's bundled executable:
  [TUN implementation](https://github.com/MetaCubeX/mihomo/blob/v1.19.31/listener/sing_tun/server.go).
- Its dependency sing-tun **v0.4.24**:
  [Windows monitor](https://github.com/metacubex/sing-tun/blob/v0.4.24/monitor_windows.go),
  [Windows TUN and filters](https://github.com/metacubex/sing-tun/blob/v0.4.24/tun_windows.go),
  [system-stack firewall integration](https://github.com/metacubex/sing-tun/blob/v0.4.24/stack_system_windows.go).
- Official WireGuard for Windows:
  [firewall rules](https://git.zx2c4.com/wireguard-windows/tree/tunnel/firewall/rules.go).
  WireGuard is a different VPN implementation; its complete filter policy cannot
  simply be pasted into a Mihomo client.

## Findings and resulting decisions

| Area | Other implementation | Atlas 15.1 | Decision |
|---|---|---|---|
| Physical interface changes | sing-tun subscribes to Windows route/interface callbacks, selects an up/connected physical interface using route plus interface metrics; Mihomo flushes interface cache and resets resolver connections | A second polling loop reapplies the entire configuration on route changes or a gap over 30 seconds | Remove the second reload policy. Let the existing core monitor handle the physical network |
| TUN identity | Interface identity is separate from physical-network changes | A changed LUID triggers full configuration reload plus filter install | Rebind only Atlas-owned WFP filters when LUID changes. A missing TUN or unresponsive core must fail health checks and enter recovery |
| TCP stack | Clash Verge's inspected default is gvisor. sing-tun's system stack installs an inbound TCP firewall allowance for the executable | mixed stack uses the system TCP path | Use gvisor, already compiled into the bundled core. This removes that system-stack dependency; it does not prove the reported outage is fixed |
| Filtering | sing-tun strict-route installs DNS restrictions and IPv6 restrictions when appropriate. WireGuard additionally has DHCP and NDP policy for its own kill switch | Atlas adds a broad outbound block above its core/TUN exceptions | Do not equate strict-route with Atlas's blanket policy. Do not silently remove protection and call that a stability fix |
| Lifecycle | Clash Verge serializes lifecycle transitions and tracks readiness generations | Atlas serializes broker commands but additionally reloads during normal network transitions | Keep bounded recovery for actual core/TUN failures, avoid making ordinary route changes into configuration replacement |

The WireGuard comparison confirms that network-control traffic needs deliberate
treatment: DHCPv4/v6 and ICMPv6 neighbor/router discovery are handled separately.
Atlas 15.1 has DHCP outbound exceptions but no NDP-specific exception. However,
WireGuard also filters inbound traffic and Atlas's custom guard currently does
not: absence of a matching inbound exception alone does not establish an Atlas
defect. Mihomo's separate IPv6 filters also have to be accounted for. The combined
installed policy must be inspected before claiming a packet was blocked by Atlas.

## Changes made after this comparison

1. gvisor for Atlas TUN, preserving strict-route and the existing DNS policy.
2. No core.apply on physical route changes or elapsed-time resume heuristics.
3. WFP-only rebind when the TUN LUID actually changes, with bounded failed-health
   recovery if the interface is missing or rebinding fails.
4. Regression coverage for unchanged, replaced and missing TUN identities; the
   generated gvisor configuration is validated by the bundled Mihomo tests.

No installed executable, adapter, route, DNS setting, or running VPN was changed.
These changes correct identified architectural differences. They are not a
verified root-cause fix for the remote office outage and have not been released.

## What remains to establish the cause

Capture the failure on the affected PC, distinguishing gateway/LAN reachability,
DNS resolution, an HTTPS request through TUN and one through the localhost proxy,
and core/controller liveness. Record the contemporaneous WFP filter identity if
Windows reports a local block. Preserve evidence before automatic core cleanup
removes the current session. Do not infer DHCP failure from a lost internet
connection when its lease is still valid and the DHCP log has no renewal errors.

Sustained internet through a failed remote VPN server cannot be guaranteed by
local TUN configuration. Manual-server failure, AUTO/FAILOVER selection, adapter
failure and intentional fail-closed blocking are separate cases. A 100% guarantee
without a captured failure and a repeatable acceptance test would be unsupported.
