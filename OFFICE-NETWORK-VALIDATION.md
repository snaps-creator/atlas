# Office network regression — 2026-09-21

## Evidence and limits

On the investigated PC, Ethernet has a valid route to its /24 LAN and the gateway
is reachable in the neighbor table. The DHCP lease duration is two hours. Windows
DHCP-Client/Admin records renewal failures (1003) at 17:29 and 18:29 and an address
acquisition failure (1001) at 15:45. The user reports that all connectivity fails
until reboot. Ping to the gateway returns General failure.

The Atlas WFP policy previously allowed only its core executable, loopback and
TUN, with a blanket block for other outgoing connections. It had no DHCP or LAN
exception. This is a concrete compatibility defect, but attributing the observed
DHCP failures to an individual installed filter still requires elevated WFP
diagnostics. No live filter changes or VPN restart were performed.

## Change

Permit UDP 68 -> 67 for DHCPv4 and UDP 546 -> 547 for DHCPv6. Permit destinations
in RFC1918 IPv4 ranges (10/8, 172.16/12, 192.168/16), allowing Windows' existing
LAN routes to work. This intentionally exempts private IPv4 destinations from
Atlas's outbound guard. It does not add routes for remote private networks.
Public addresses and the synthetic 198.19/16 range receive no new exception.
Filters are installed atomically and removed with the dynamic WFP session.

DNS leak protection remains enabled. This change does not implement corporate
split DNS, IPv6 LAN access or multicast service discovery. If internal DNS names
are used, collect the office DNS suffixes and authoritative resolver addresses
and implement explicit nameserver policies plus fake-IP exclusions. Do not
disable strict routing globally or send corporate names to public resolvers.

## Required acceptance before office rollout

Run only in an agreed maintenance window; installing the build requires restarting
Atlas. Do not renew/release DHCP or restart adapters on an active user session.

1. Test on two PCs simultaneously in the same LAN. Verify gateway, file server,
   printer, internet and intended application routing in rule/global/direct modes.
2. Keep both PCs connected for longer than the complete DHCP lease (over two hours
   in the observed office). Verify lease renewal timestamps advance and there are
   no new DHCP events 1001/1003. Repeat through another renewal cycle.
3. Test VPN server loss: public internet must remain blocked according to the
   protection policy while DHCP and private IPv4 LAN access continue working.
4. In a maintenance test, verify disconnect/reconnect, sleep/resume, changing the
   physical interface, and app/service exit clean up or reinstall owned filters.
5. Verify corporate DNS names separately if present. Do not infer DNS or IPv6 LAN
   support from a successful gateway ping.

The same virtual TUN address on separate PCs is expected: it is not an address
advertised on their physical LAN. No per-PC TUN address randomization is needed.
