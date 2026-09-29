# Local Atlas 2.1.3-alpha.1

The local build adds a pinned Xray 26.3.27 worker alongside Mihomo. The source archive and all bundled Xray assets are pinned in `src-tauri/resources/xray-manifest.json`. Nothing in this change publishes a GitHub release.

- VLESS URI links use Xray. RAW/TCP, WebSocket, gRPC, HTTPUpgrade, XHTTP/SplitHTTP and mKCP are supported. TLS/REALITY, fingerprints, ALPN, XHTTP extra settings and VLESS encryption/flow are retained.
- URI `fragment=length,interval,packets[,maxSplit]` and `noises=type,packet,delay[,applyTo]` configure the actual Xray transport dialer. Provider fragmentation/noise headers are also applied. Explicit URI settings take priority. These options are not enabled indiscriminately: they come from the subscription.
- Xray JSON objects and arrays of client profiles are imported, including DNS, routing and multiple outbounds. Atlas replaces public inbound/API listeners and file logging with authenticated loopback listeners and its own logs. Host file references, reverse-server profiles and caller-selected network interfaces are rejected; WireGuard uses userspace. This is client-profile import, not permission to run an arbitrary server configuration as SYSTEM.
- URI profiles share one Xray process. Complete JSON profiles retain independent DNS/routing contexts. Resource limits are 512 Xray nodes and 16 independent JSON profiles per session.
- Provider headers/body directives support fallback URLs, provider-authorized URL/domain migration, refresh interval and User-Agent. The default compatible User-Agent is `Happ/4.3.0`; a provider directive or per-subscription override takes precedence. This identifies the requested subscription format, not the application vendor.
- Each Xray process has an Atlas name and an owned Windows Job Object. Local SOCKS listeners require random credentials. Cancellation interrupts startup; shutdown signals all workers together. Network policy permits the exact bundled Xray path. Windows outbound sockets bind to the default-route interface so fragmentation is not reassembled by Atlas's TUN. DNS bootstrap uses direct DoH with literal resolver addresses.

Validation uses real bundled cores and loopback VLESS/HTTP servers without modifying Windows adapters, proxy, DNS, routes or firewall. Tests cover all six transports, authenticated forwarding, selection, reload and cleanup. Production-provider reachability and physical TUN/WFP operation are not established by loopback tests. No test here installs Atlas or changes the user's working network.

Upstream references: https://www.happ.su/main/dev-docs/app-management , https://www.happ.su/main/dev-docs/examples-of-links-and-parameters , https://github.com/XTLS/Xray-core/releases/tag/v26.3.27 .
