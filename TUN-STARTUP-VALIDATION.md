# TUN startup validation

## Root causes and changes

Mihomo can expose its controller before its TUN adapter is ready. Atlas now
waits for readiness while the core remains alive, up to a bounded deadline.
An explicit TUN-listener error stops this wait immediately. Both stdout and
stderr are retained for diagnosis.

The real Beta 10 connection failed with `set ipv4 address: The object already
exists`. The other Mihomo adapter already owned 198.18.0.1/30, which Atlas also
inherited from the default fake-ip-range. Atlas now uses 198.19.0.1/16 and the
separate IPv6 prefix fd72:6174:6c61::1/126. User routing YAML is unchanged.

## Isolated Windows test, 21 September 2026

The real bundled core created Atlas-TUN at 198.19.0.1/30 while the existing
Mihomo adapter retained 198.18.0.1/30. Atlas-TUN reached Up; it disappeared
when the probe ended. Before/after default-route snapshots were identical.
The probe disabled auto-routing, DNS listening and WFP protection.

Earlier timing probes observed controller startup about 250 ms before TUN
readiness. The first captured /configs response already showed TUN enabled;
a false-to-true transition was not captured. The timing evidence alone does
not reproduce the full user failure.

Full installed-backend checks and remaining limits are recorded in
[ROUTE-DIAGNOSTICS-VALIDATION.md](ROUTE-DIAGNOSTICS-VALIDATION.md).
