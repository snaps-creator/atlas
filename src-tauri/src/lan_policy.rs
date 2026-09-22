//! One policy for TUN route exclusions and the additional Windows guard.
//! These are local scopes, not a general bypass for public destinations.
pub(crate) const PREFIXES: &[&str] = &[
    "10.0.0.0/8",
    "172.16.0.0/12",
    "192.168.0.0/16",
    "169.254.0.0/16",
    "224.0.0.0/24",
    "239.255.0.0/16",
    "fc00::/7",
    "fe80::/10",
    "ff02::/16",
];

pub(crate) fn route_exclusions() -> Vec<&'static str> {
    // The guard permits limited broadcast only through its DHCP port rule.
    PREFIXES
        .iter()
        .copied()
        .chain(["255.255.255.255/32"])
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn local_address_maintenance_and_discovery_stay_outside_tun() {
        let prefixes: Vec<ipnet::IpNet> = route_exclusions()
            .iter()
            .map(|s| s.parse().unwrap())
            .collect();
        for address in [
            "192.168.1.1",
            "10.1.2.3",
            "172.16.1.1",
            "169.254.1.2",
            "255.255.255.255",
            "224.0.0.251",
            "239.255.255.250",
            "fe80::1",
            "fd12::1",
            "ff02::1:2",
            "ff02::1:ff00:1",
            "ff02::2",
        ] {
            let ip: std::net::IpAddr = address.parse().unwrap();
            assert!(prefixes.iter().any(|p| p.contains(&ip)), "{address}");
        }
        for address in [
            "1.1.1.1",
            "8.8.8.8",
            "198.19.0.1",
            "172.32.0.1",
            "2606:4700:4700::1111",
            "ff0e::1",
        ] {
            let ip: std::net::IpAddr = address.parse().unwrap();
            assert!(!prefixes.iter().any(|p| p.contains(&ip)), "{address}");
        }
    }
}
