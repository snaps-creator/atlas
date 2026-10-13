//! Read-only uplink observation. Native API calls never own the service lock.
use std::{
    sync::mpsc::{self, Receiver},
    time::{Duration, Instant},
};

#[derive(Clone, PartialEq, Eq)]
struct Snapshot {
    network: String,
    wifi: Option<String>,
    online: bool,
}
#[derive(Default)]
struct Changes {
    stable: Option<Snapshot>,
    candidate: Option<(Snapshot, Instant)>,
    last_sample: Option<Instant>,
}
impl Changes {
    fn observe(&mut self, sample: Option<Snapshot>, now: Instant) -> bool {
        let Some(mut sample) = sample else {
            self.candidate = None;
            return false;
        };
        // Windows may deny SSID access. Missing optional evidence is not a
        // network change; IP/DNS/default-route monitoring remains operational.
        if sample.wifi.is_none() {
            sample.wifi = self.stable.as_ref().and_then(|s| s.wifi.clone());
        }
        let resumed = self
            .last_sample
            .is_some_and(|last| now.duration_since(last) > Duration::from_secs(30));
        self.last_sample = Some(now);
        let Some(stable) = &self.stable else {
            self.stable = Some(sample);
            return false;
        };
        if *stable == sample {
            self.candidate = None;
            return resumed && sample.online;
        }
        if self.candidate.as_ref().is_some_and(|(value, since)| {
            *value == sample && now.duration_since(*since) >= Duration::from_secs(2)
        }) {
            let online = sample.online;
            self.stable = Some(sample);
            self.candidate = None;
            return online;
        }
        if self
            .candidate
            .as_ref()
            .is_none_or(|(value, _)| *value != sample)
        {
            self.candidate = Some((sample, now));
        }
        false
    }
}

#[derive(Default)]
pub(crate) struct Monitor {
    changes: Changes,
    pending: Option<Receiver<Option<Snapshot>>>,
    last_started: Option<Instant>,
    abandoned: Option<Receiver<Option<Snapshot>>>,
}
static WORKERS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
struct WorkerSlot;
impl WorkerSlot {
    fn take() -> Option<Self> {
        WORKERS
            .fetch_update(
                std::sync::atomic::Ordering::SeqCst,
                std::sync::atomic::Ordering::SeqCst,
                |count| (count < 4).then_some(count + 1),
            )
            .ok()
            .map(|_| Self)
    }
}
impl Drop for WorkerSlot {
    fn drop(&mut self) {
        WORKERS.fetch_sub(1, std::sync::atomic::Ordering::SeqCst);
    }
}
impl Monitor {
    fn retire_stalled(&mut self, now: Instant) {
        if self
            .abandoned
            .as_ref()
            .is_some_and(|rx| !matches!(rx.try_recv(), Err(mpsc::TryRecvError::Empty)))
        {
            self.abandoned = None; // Late sample belongs to an obsolete observation.
        }
        if self.abandoned.is_none()
            && self.pending.is_some()
            && self
                .last_started
                .is_some_and(|at| now.duration_since(at) > Duration::from_secs(15))
        {
            self.abandoned = self.pending.take();
        }
    }
    pub(crate) fn online(&self) -> Option<bool> {
        if self.stale() {
            return None;
        }
        self.changes.stable.as_ref().map(|s| s.online)
    }
    pub(crate) fn stale(&self) -> bool {
        self.changes
            .last_sample
            .is_some_and(|at| at.elapsed() > Duration::from_secs(15))
            || (self.pending.is_some()
                && self
                    .last_started
                    .is_some_and(|at| at.elapsed() > Duration::from_secs(15)))
    }
    pub(crate) fn update_warning(&self, error: &mut Option<String>) {
        const STALE: &str = "Наблюдение сети задерживается; состояние uplink неизвестно";
        if self.stale() {
            if error.is_none() {
                *error = Some(STALE.into());
            }
        } else if error.as_deref() == Some(STALE) {
            // Disconnected desktops have no service query to clear this warning.
            // A fresh observation clears only the warning owned by this monitor.
            *error = None;
        }
    }
    pub(crate) fn tick(&mut self) -> bool {
        let now = Instant::now();
        let mut changed = false;
        if let Some(rx) = &self.pending {
            match rx.try_recv() {
                Ok(sample) => {
                    self.pending = None;
                    changed = self.changes.observe(sample, now);
                }
                Err(mpsc::TryRecvError::Disconnected) => self.pending = None,
                Err(mpsc::TryRecvError::Empty) => {}
            }
        }
        self.retire_stalled(now);
        if self.pending.is_none()
            && self
                .last_started
                .is_none_or(|at| now.duration_since(at) >= Duration::from_secs(2))
        {
            let Some(slot) = WorkerSlot::take() else {
                return changed;
            };
            let (tx, rx) = mpsc::channel();
            if std::thread::Builder::new()
                .name("atlas-uplink-observer".into())
                .spawn(move || {
                    let _slot = slot;
                    let _ = tx.send(snapshot());
                })
                .is_ok()
            {
                self.pending = Some(rx);
                self.last_started = Some(now);
            }
        }
        changed
    }
}

fn digest(parts: &[String]) -> String {
    use sha2::{Digest, Sha256};
    let mut parts = parts.to_vec();
    parts.sort();
    parts.dedup();
    format!("{:x}", Sha256::digest(parts.join("\n").as_bytes()))
}

fn snapshot() -> Option<Snapshot> {
    use windows_sys::Win32::{
        Foundation::ERROR_BUFFER_OVERFLOW,
        NetworkManagement::{IpHelper::*, Ndis::IfOperStatusUp},
        Networking::WinSock::*,
    };
    let routes = crate::network_guard::default_route_signature()?;
    let uplinks: std::collections::HashSet<u64> = routes
        .split('|')
        .filter_map(|r| r.split(':').next()?.parse().ok())
        .collect();
    if uplinks.is_empty() {
        return Some(Snapshot {
            network: digest(&[routes]),
            wifi: None,
            online: false,
        });
    }
    let mut network = vec![routes];
    let mut wifi = Vec::new();
    let mut uplink_ready = false;
    // u64 storage provides the documented alignment; Windows may request a
    // larger buffer if an adapter appeared between enumeration calls.
    let mut size = 16384u32;
    for _ in 0..3 {
        if size > 4 * 1024 * 1024 {
            return None;
        }
        let mut buffer = vec![0u64; (size as usize).div_ceil(8)];
        let head = buffer.as_mut_ptr().cast::<IP_ADAPTER_ADDRESSES_LH>();
        let status = unsafe {
            GetAdaptersAddresses(
                AF_UNSPEC as u32,
                GAA_FLAG_SKIP_ANYCAST | GAA_FLAG_SKIP_MULTICAST | GAA_FLAG_SKIP_FRIENDLY_NAME,
                std::ptr::null(),
                head,
                &mut size,
            )
        };
        if status == ERROR_BUFFER_OVERFLOW {
            continue;
        }
        if status != 0 {
            return None;
        }
        unsafe {
            let mut cursor = head;
            while let Some(adapter) = cursor.as_ref() {
                let luid = adapter.Luid.Value;
                if adapter.OperStatus == IfOperStatusUp && uplinks.contains(&luid) {
                    let g = adapter.NetworkGuid;
                    network.push(format!(
                        "adapter:{luid}:{}:{:x}:{:x}:{:x}:{:x?}",
                        adapter.Mtu, g.data1, g.data2, g.data3, g.data4
                    ));
                    let mut address = adapter.FirstUnicastAddress;
                    while let Some(a) = address.as_ref() {
                        if a.DadState == IpDadStatePreferred {
                            if let Some(ip) = socket_ip(&a.Address) {
                                uplink_ready = true;
                                network.push(format!("ip:{luid}:{ip}/{}", a.OnLinkPrefixLength));
                            }
                        }
                        address = a.Next;
                    }
                    let mut dns = adapter.FirstDnsServerAddress;
                    let mut order = 0;
                    while let Some(d) = dns.as_ref() {
                        if let Some(ip) = socket_ip(&d.Address) {
                            network.push(format!("dns:{luid}:{order}:{ip}"));
                        }
                        order += 1;
                        dns = d.Next;
                    }
                    if adapter.IfType == IF_TYPE_IEEE80211 {
                        if let Some(id) = wifi_identity(&adapter.Luid) {
                            wifi.push(format!("{luid}:{id}"));
                        }
                    }
                }
                cursor = adapter.Next;
            }
        }
        // Neither SSIDs nor addresses are emitted in incident events.
        return Some(Snapshot {
            network: digest(&network),
            wifi: (!wifi.is_empty()).then(|| digest(&wifi)),
            online: uplink_ready,
        });
    }
    None
}

unsafe fn socket_ip(
    address: &windows_sys::Win32::Networking::WinSock::SOCKET_ADDRESS,
) -> Option<String> {
    use windows_sys::Win32::Networking::WinSock::*;
    if address.lpSockaddr.is_null() || address.iSockaddrLength < 2 {
        return None;
    }
    match (*address.lpSockaddr).sa_family {
        AF_INET if address.iSockaddrLength as usize >= std::mem::size_of::<SOCKADDR_IN>() => {
            let a = &*address.lpSockaddr.cast::<SOCKADDR_IN>();
            Some(format!("v4:{:x}", a.sin_addr.S_un.S_addr))
        }
        AF_INET6 if address.iSockaddrLength as usize >= std::mem::size_of::<SOCKADDR_IN6>() => {
            let a = &*address.lpSockaddr.cast::<SOCKADDR_IN6>();
            Some(format!(
                "v6:{:x?}:{}",
                a.sin6_addr.u.Byte, a.Anonymous.sin6_scope_id
            ))
        }
        _ => None,
    }
}

unsafe fn wifi_identity(
    luid: &windows_sys::Win32::NetworkManagement::Ndis::NET_LUID_LH,
) -> Option<String> {
    use windows_sys::Win32::NetworkManagement::{IpHelper::ConvertInterfaceLuidToGuid, WiFi::*};
    let mut guid = std::mem::zeroed();
    if ConvertInterfaceLuidToGuid(luid, &mut guid) != 0 {
        return None;
    }
    let mut handle = std::ptr::null_mut();
    let mut version = 0;
    if WlanOpenHandle(2, std::ptr::null(), &mut version, &mut handle) != 0 {
        return None;
    }
    let mut data = std::ptr::null_mut();
    let mut size = 0;
    let result = WlanQueryInterface(
        handle,
        &guid,
        wlan_intf_opcode_current_connection,
        std::ptr::null(),
        &mut size,
        &mut data,
        std::ptr::null_mut(),
    );
    let value = if result == 0
        && !data.is_null()
        && size as usize >= std::mem::size_of::<WLAN_CONNECTION_ATTRIBUTES>()
    {
        let connection = &*data.cast::<WLAN_CONNECTION_ATTRIBUTES>();
        let a = &connection.wlanAssociationAttributes;
        (connection.isState == wlan_interface_state_connected).then(|| {
            format!(
                "{:x?}:{:x?}",
                &a.dot11Ssid.ucSSID[..(a.dot11Ssid.uSSIDLength as usize).min(32)],
                a.dot11Bssid
            )
        })
    } else {
        None
    };
    if !data.is_null() {
        WlanFreeMemory(data);
    }
    WlanCloseHandle(handle, std::ptr::null());
    value
}

#[cfg(test)]
mod tests {
    use super::*;
    fn sample(network: &str, wifi: Option<&str>) -> Option<Snapshot> {
        Some(Snapshot {
            network: network.into(),
            wifi: wifi.map(str::to_owned),
            online: true,
        })
    }
    #[test]
    fn wifi_change_with_identical_ip_and_gateway_is_debounced_once() {
        let mut changes = Changes::default();
        let t = Instant::now();
        assert!(!changes.observe(sample("same-address", "office".into()), t));
        assert!(!changes.observe(
            sample("same-address", "home".into()),
            t + Duration::from_secs(2)
        ));
        assert!(changes.observe(
            sample("same-address", "home".into()),
            t + Duration::from_secs(4)
        ));
        assert!(!changes.observe(
            sample("same-address", "home".into()),
            t + Duration::from_secs(6)
        ));
    }
    #[test]
    fn unchanged_network_errors_and_transient_disconnect_do_not_restart() {
        let mut changes = Changes::default();
        let t = Instant::now();
        assert!(!changes.observe(sample("office", Some("wifi")), t));
        assert!(!changes.observe(None, t + Duration::from_secs(2)));
        assert!(!changes.observe(sample("offline", None), t + Duration::from_secs(4)));
        assert!(!changes.observe(sample("office", None), t + Duration::from_secs(6)));
        assert!(!changes.observe(sample("office", Some("wifi")), t + Duration::from_secs(8)));
    }
    #[test]
    fn dns_ip_change_and_wake_trigger_fresh_path_validation() {
        let mut changes = Changes::default();
        let t = Instant::now();
        assert!(!changes.observe(sample("old", None), t));
        assert!(!changes.observe(sample("new", None), t + Duration::from_secs(2)));
        assert!(changes.observe(sample("new", None), t + Duration::from_secs(4)));
        assert!(changes.observe(sample("new", None), t + Duration::from_secs(65)));
        assert!(!changes.observe(sample("new", None), t + Duration::from_secs(67)));
    }
    #[test]
    fn address_order_does_not_change_fingerprint() {
        assert_eq!(
            digest(&["a".into(), "b".into()]),
            digest(&["b".into(), "a".into()])
        );
        assert_ne!(
            digest(&["dns:0:a".into(), "dns:1:b".into()]),
            digest(&["dns:0:b".into(), "dns:1:a".into()])
        );
    }
    #[test]
    fn missing_uplink_waits_for_a_stable_return_before_requesting_recovery() {
        let mut changes = Changes::default();
        let t = Instant::now();
        changes.observe(sample("office", None), t);
        let offline = Some(Snapshot {
            network: "offline".into(),
            wifi: None,
            online: false,
        });
        assert!(!changes.observe(offline.clone(), t + Duration::from_secs(2)));
        assert!(!changes.observe(offline, t + Duration::from_secs(4)));
        assert!(!changes.observe(sample("office", None), t + Duration::from_secs(6)));
        assert!(changes.observe(sample("office", None), t + Duration::from_secs(8)));
    }
    #[test]
    fn native_snapshot_is_read_only_and_available() {
        assert!(snapshot().is_some());
    }
    #[test]
    fn stalled_windows_query_does_not_block_commands_or_spawn_another_worker() {
        let (_tx, rx) = mpsc::channel();
        let mut monitor = Monitor {
            pending: Some(rx),
            ..Default::default()
        };
        let started = Instant::now();
        for _ in 0..1000 {
            assert!(!monitor.tick());
            assert!(monitor.pending.is_some());
        }
        assert!(started.elapsed() < Duration::from_secs(1));
    }
    #[test]
    fn stalled_observer_is_explicitly_unknown_and_keeps_one_worker() {
        let (_tx, rx) = mpsc::channel();
        let mut monitor = Monitor {
            pending: Some(rx),
            last_started: Some(Instant::now() - Duration::from_secs(20)),
            ..Default::default()
        };
        monitor.changes.stable = sample("old", None);
        assert!(monitor.stale());
        assert_eq!(monitor.online(), None);
        monitor.retire_stalled(Instant::now());
        assert!(monitor.abandoned.is_some());
        assert!(monitor.pending.is_none());
        let (_second, rx) = mpsc::channel();
        monitor.pending = Some(rx);
        monitor.last_started = Some(Instant::now() - Duration::from_secs(20));
        for _ in 0..100 {
            monitor.retire_stalled(Instant::now());
            assert!(monitor.stale());
        }
    }
    #[test]
    fn fresh_observation_clears_only_its_stale_warning_without_a_service_query() {
        let mut monitor = Monitor::default();
        monitor.changes.last_sample = Some(Instant::now() - Duration::from_secs(20));
        let mut warning = None;
        monitor.update_warning(&mut warning);
        assert!(warning.is_some());
        monitor.changes.last_sample = Some(Instant::now());
        monitor.update_warning(&mut warning);
        assert_eq!(warning, None);
        warning = Some("credential reconciliation failed".into());
        monitor.update_warning(&mut warning);
        assert_eq!(warning.as_deref(), Some("credential reconciliation failed"));
        monitor.changes.last_sample = Some(Instant::now() - Duration::from_secs(20));
        monitor.update_warning(&mut warning);
        assert_eq!(warning.as_deref(), Some("credential reconciliation failed"));
    }
}
