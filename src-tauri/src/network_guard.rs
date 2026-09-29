//! Atlas-owned session WFP policy. Only the owned core, loopback and the TUN interface
//! can originate internet traffic. DHCP and local IPv4/IPv6 destinations are
//! permitted so that the host can keep its address and reach office resources.
//! The dynamic WFP session disappears when the service exits, including a hard
//! crash. Legacy persistent filters are removed by their exact Atlas keys.
use std::{path::Path, ptr, sync::Mutex};
use windows_sys::{
    core::GUID,
    Win32::{
        Foundation::HANDLE,
        NetworkManagement::{
            IpHelper::{ConvertInterfaceAliasToLuid, ConvertInterfaceLuidToGuid},
            Ndis::NET_LUID_LH, WindowsFilteringPlatform::*,
        },
    },
};
const LEGACY_SUBLAYER: GUID = GUID::from_u128(0xe12a8041_d824_4776_978b_31a129691c00);
const SESSION_SUBLAYER: GUID = GUID::from_u128(0xe12a8041_d824_4776_978b_31a129691c01);
const LEGACY_FILTER_BASE: u128 = 0xe12a8041_d824_4776_978b_31a129692000;
const SESSION_FILTER_BASE: u128 = 0xe12a8041_d824_4776_978b_31a129693000;
fn key(index: u128) -> GUID {
    GUID::from_u128(SESSION_FILTER_BASE + index)
}
fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(Some(0)).collect()
}
fn checked(code: u32, op: &str) -> Result<(), String> {
    if code == 0 {
        Ok(())
    } else {
        Err(format!("Защита сети: {op}, код Windows {code:#x}"))
    }
}
struct Engine(HANDLE);
impl Engine {
    fn open(dynamic: bool) -> Result<Self, String> {
        unsafe {
            let mut h = ptr::null_mut();
            let mut session: FWPM_SESSION0 = std::mem::zeroed();
            session.flags = if dynamic {
                FWPM_SESSION_FLAG_DYNAMIC
            } else {
                0
            };
            checked(
                FwpmEngineOpen0(ptr::null(), 10, ptr::null(), &session, &mut h),
                "открытие WFP",
            )?;
            Ok(Self(h))
        }
    }
}
impl Drop for Engine {
    fn drop(&mut self) {
        unsafe {
            FwpmEngineClose0(self.0);
        }
    }
}
unsafe fn erase(engine: HANDLE, sublayer: &GUID, filter_base: u128) -> Result<(), String> {
    for i in 0..(12 + crate::lan_policy::PREFIXES.len() as u128) {
        let r = FwpmFilterDeleteByKey0(engine, &GUID::from_u128(filter_base + i));
        if r != 0 && r != 0x80320003 {
            return Err(format!("Не удалось удалить фильтр Атласа: {r:#x}"));
        }
    }
    let r = FwpmSubLayerDeleteByKey0(engine, sublayer);
    if r != 0 && r != 0x80320007 {
        return Err(format!("Не удалось удалить слой Атласа: {r:#x}"));
    }
    Ok(())
}
pub fn clear() -> Result<(), String> {
    // Migration and uninstall only: delete the exact old Atlas-owned keys.
    // Current session filters are lifetime-bound to their service handle.
    let e = Engine::open(false)?;
    unsafe {
        checked(FwpmTransactionBegin0(e.0, 0), "транзакция")?;
        if let Err(err) = erase(e.0, &LEGACY_SUBLAYER, LEGACY_FILTER_BASE) {
            FwpmTransactionAbort0(e.0);
            return Err(err);
        }
        checked(FwpmTransactionCommit0(e.0), "завершение транзакции")
    }
}
pub struct Guard {
    engine: Engine,
    owned_tun: Mutex<Option<(u64, u128)>>,
}

fn guid_value(guid: GUID) -> u128 {
    ((guid.data1 as u128) << 96)
        | ((guid.data2 as u128) << 80)
        | ((guid.data3 as u128) << 64)
        | u64::from_be_bytes(guid.data4) as u128
}

pub fn tun_identity() -> Option<u64> {
    unsafe {
        let mut tun: NET_LUID_LH = std::mem::zeroed();
        (ConvertInterfaceAliasToLuid(wide("Atlas-TUN").as_ptr(), &mut tun) == 0)
            .then_some(tun.Value)
    }
}

pub(crate) fn wait_for_tun_release(timeout: std::time::Duration) -> Result<(), String> {
    wait_for_tun_release_with(timeout, tun_identity)
}
fn wait_for_tun_release_with(timeout: std::time::Duration, identity: impl Fn() -> Option<u64>) -> Result<(), String> {
    let deadline = std::time::Instant::now() + timeout;
    while identity().is_some() {
        if std::time::Instant::now() >= deadline {
            return Err("Atlas-TUN остался после остановки службы. Очистка не подтверждена; установка или удаление остановлены. Неизвестный интерфейс не изменён".into());
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    Ok(())
}

/// Stable identity of physical default routes. Changing Wi-Fi, gateway or VPN
/// uplink starts a new health epoch without treating the old network's failed
/// probes as evidence against servers on the new one.
pub fn default_route_signature() -> Option<String> {
    use windows_sys::Win32::{
        NetworkManagement::IpHelper::{FreeMibTable, GetIpForwardTable2},
        Networking::WinSock::{AF_INET, AF_INET6, AF_UNSPEC},
    };
    unsafe {
        let mut table = ptr::null_mut();
        if GetIpForwardTable2(AF_UNSPEC, &mut table) != 0 { return None; }
        let rows = std::slice::from_raw_parts((*table).Table.as_ptr(), (*table).NumEntries as usize);
        let tun = tun_identity();
        let mut routes: Vec<String> = rows.iter().filter(|r|
            r.DestinationPrefix.PrefixLength == 0 && r.Loopback == 0 && Some(r.InterfaceLuid.Value) != tun
        ).map(|r| {
            let family = r.NextHop.si_family;
            let gateway = if family == AF_INET {
                format!("{:08x}", r.NextHop.Ipv4.sin_addr.S_un.S_addr)
            } else if family == AF_INET6 {
                format!("{:02x?}", r.NextHop.Ipv6.sin6_addr.u.Byte)
            } else { String::new() };
            format!("{}:{}:{}", r.InterfaceLuid.Value, r.Metric, gateway)
        }).collect();
        FreeMibTable(table.cast());
        routes.sort();
        Some(routes.join("|"))
    }
}

/// A competing full-tunnel must be disconnected by its owner before Atlas connects.
/// Reading routes has no side effects; never delete another client's routes/filters.
pub fn check_competing_routes() -> Result<(), String> {
    use windows_sys::Win32::{
        NetworkManagement::IpHelper::{FreeMibTable, GetIpForwardTable2},
        Networking::WinSock::AF_UNSPEC,
    };
    if tun_identity().is_some() {
        return Err("Интерфейс Atlas-TUN уже существует до запуска ядра. Его происхождение не подтверждено; Atlas не будет его менять".into());
    }
    unsafe {
        let mut table = ptr::null_mut();
        checked(
            GetIpForwardTable2(AF_UNSPEC, &mut table),
            "проверка маршрутов перед подключением",
        )?;
        let rows =
            std::slice::from_raw_parts((*table).Table.as_ptr(), (*table).NumEntries as usize);
        let conflict = rows
            .iter()
            .any(|row| row.DestinationPrefix.PrefixLength == 1 && row.Loopback == 0);
        FreeMibTable(table.cast());
        if conflict {
            return Err("Обнаружен активный маршрут другого VPN. Отключите его перед подключением Atlas. Сетевые настройки не изменены.".into());
        }
    }
    Ok(())
}

impl Guard {
    pub fn prepare(core: &Path) -> Result<Self, String> {
        if tun_identity().is_some() {
            return Err("Atlas-TUN появился до запуска ядра; его происхождение не подтверждено".into());
        }
        let guard = Self { engine: Engine::open(true)?, owned_tun: Mutex::new(None) };
        // The protected pause exists only while this service session lives.
        guard.policy(core, false)?;
        Ok(guard)
    }
    pub fn install(&self, core: &Path) -> Result<(), String> {
        self.policy(core, true)
    }
    pub fn pause(&self, core: &Path) -> Result<(), String> {
        self.policy(core, false)
    }
    pub fn reset_for_recovery(&self) -> Result<(), String> {
        if tun_identity().is_some() {
            return Err("Старый Atlas-TUN ещё существует; восстановление остановлено".into());
        }
        *self.owned_tun.lock().map_err(|_| "Не удалось сбросить идентификатор TUN")? = None;
        Ok(())
    }
    fn policy(&self, core: &Path, tun_ready: bool) -> Result<(), String> {
        let e = &self.engine;
        unsafe {
            let mut luid: NET_LUID_LH = std::mem::zeroed();
            let mut tun_instance = None;
            if tun_ready {
                checked(
                    ConvertInterfaceAliasToLuid(wide("Atlas-TUN").as_ptr(), &mut luid),
                    "интерфейс Atlas-TUN не найден",
                )?;
                let mut guid: GUID = std::mem::zeroed();
                checked(ConvertInterfaceLuidToGuid(&luid, &mut guid), "GUID Atlas-TUN не найден")?;
                tun_instance = Some((luid.Value, guid_value(guid)));
                if self.owned_tun.lock().map_err(|_| "Не удалось проверить идентификатор TUN")?
                    .is_some_and(|owned| Some(owned) != tun_instance) {
                    return Err("Идентификатор Atlas-TUN изменился: запрещено привязывать защиту к неподтверждённому интерфейсу".into());
                }
            }
            let mut app_id = ptr::null_mut();
            checked(
                FwpmGetAppIdFromFileName0(wide(&core.to_string_lossy()).as_ptr(), &mut app_id),
                "идентификатор ядра",
            )?;
            let mut xray_app_id=ptr::null_mut();
            let xray_path=core.with_file_name("Atlas.Xray.exe");
            if xray_path.is_file() {
                let code=FwpmGetAppIdFromFileName0(wide(&xray_path.to_string_lossy()).as_ptr(),&mut xray_app_id);
                if code!=0 {FwpmFreeMemory0(&mut app_id as *mut _ as *mut _);return Err(format!("Идентификатор Xray: {code}"));}
            }
            let outcome = (|| {
                checked(FwpmTransactionBegin0(e.0, 0), "транзакция")?;
                erase(e.0, &SESSION_SUBLAYER, SESSION_FILTER_BASE)?;
                let mut name = wide("Атлас — защита VPN");
                let mut sub: FWPM_SUBLAYER0 = std::mem::zeroed();
                sub.subLayerKey = SESSION_SUBLAYER;
                sub.displayData.name = name.as_mut_ptr();
                sub.weight = 0x100;
                checked(FwpmSubLayerAdd0(e.0, &sub, ptr::null_mut()), "слой")?;
                for (family, layer) in [
                    FWPM_LAYER_ALE_AUTH_CONNECT_V4,
                    FWPM_LAYER_ALE_AUTH_CONNECT_V6,
                ]
                .into_iter()
                .enumerate()
                {
                    if !xray_app_id.is_null() {
                        let mut condition: FWPM_FILTER_CONDITION0=std::mem::zeroed();
                        condition.fieldKey=FWPM_CONDITION_ALE_APP_ID;
                        condition.matchType=FWP_MATCH_EQUAL;
                        condition.conditionValue.r#type=FWP_BYTE_BLOB_TYPE;
                        condition.conditionValue.Anonymous.byteBlob=xray_app_id;
                        permit(e.0,(10+crate::lan_policy::PREFIXES.len()+family) as u128,layer,name.as_mut_ptr(),&mut [condition])?;
                    }
                    for kind in 0..4 {
                        if kind == 2 && !tun_ready {
                            continue;
                        }
                        let mut condition: FWPM_FILTER_CONDITION0 = std::mem::zeroed();
                        condition.matchType = FWP_MATCH_EQUAL;
                        let mut interface = luid.Value;
                        match kind {
                            0 => {
                                condition.fieldKey = FWPM_CONDITION_ALE_APP_ID;
                                condition.conditionValue.r#type = FWP_BYTE_BLOB_TYPE;
                                condition.conditionValue.Anonymous.byteBlob = app_id;
                            }
                            1 => {
                                condition.fieldKey = FWPM_CONDITION_FLAGS;
                                condition.matchType = FWP_MATCH_FLAGS_ALL_SET;
                                condition.conditionValue.r#type = FWP_UINT32;
                                condition.conditionValue.Anonymous.uint32 =
                                    FWP_CONDITION_FLAG_IS_LOOPBACK;
                            }
                            2 => {
                                condition.fieldKey = FWPM_CONDITION_IP_LOCAL_INTERFACE;
                                condition.conditionValue.r#type = FWP_UINT64;
                                condition.conditionValue.Anonymous.uint64 = &mut interface;
                            }
                            _ => {}
                        }
                        let mut filter: FWPM_FILTER0 = std::mem::zeroed();
                        filter.filterKey = key((family * 4 + kind) as u128);
                        filter.displayData.name = name.as_mut_ptr();
                        filter.layerKey = layer;
                        filter.subLayerKey = SESSION_SUBLAYER;
                        filter.weight.r#type = FWP_UINT8;
                        filter.weight.Anonymous.uint8 = if kind == 3 { 1 } else { 10 };
                        filter.action.r#type = if kind == 3 {
                            FWP_ACTION_BLOCK
                        } else {
                            FWP_ACTION_PERMIT
                        };
                        if kind != 3 {
                            filter.numFilterConditions = 1;
                            filter.filterCondition = &mut condition;
                        }
                        checked(
                            FwpmFilterAdd0(e.0, &filter, ptr::null_mut(), ptr::null_mut()),
                            "фильтр",
                        )?;
                    }
                }
                // DHCP must survive the kill switch, including broadcast discovery
                // and unicast renewal. Match protocol AND both ports, never all UDP.
                for (offset, layer, local, remote) in [
                    (8, FWPM_LAYER_ALE_AUTH_CONNECT_V4, 68, 67),
                    (9, FWPM_LAYER_ALE_AUTH_CONNECT_V6, 546, 547),
                ] {
                    let mut conditions = dhcp_conditions(local, remote);
                    permit(e.0, offset, layer, name.as_mut_ptr(), &mut conditions)?;
                }
                // Local maintenance/discovery and office traffic stay on Ethernet/Wi-Fi.
                // Use the same scoped destinations as TUN, without permitting public internet
                // on those interfaces. Do not change routes or disable DNS leak filters.
                for (index, prefix) in crate::lan_policy::PREFIXES.iter().enumerate() {
                    let network: ipnet::IpNet = prefix.parse().map_err(|e| format!("LAN prefix: {e}"))?;
                    let mut condition: FWPM_FILTER_CONDITION0 = std::mem::zeroed();
                    condition.fieldKey = FWPM_CONDITION_IP_REMOTE_ADDRESS;
                    let mut v4: FWP_V4_ADDR_AND_MASK = std::mem::zeroed();
                    let mut v6: FWP_V6_ADDR_AND_MASK = std::mem::zeroed();
                    let layer = match network {
                        ipnet::IpNet::V4(n) => {
                            v4.addr = u32::from(n.network());
                            v4.mask = u32::from(n.netmask());
                            condition.conditionValue.r#type = FWP_V4_ADDR_MASK;
                            condition.conditionValue.Anonymous.v4AddrMask = &mut v4;
                            FWPM_LAYER_ALE_AUTH_CONNECT_V4
                        }
                        ipnet::IpNet::V6(n) => {
                            v6.addr = n.network().octets();
                            v6.prefixLength = n.prefix_len();
                            condition.conditionValue.r#type = FWP_V6_ADDR_MASK;
                            condition.conditionValue.Anonymous.v6AddrMask = &mut v6;
                            FWPM_LAYER_ALE_AUTH_CONNECT_V6
                        }
                    };
                    permit(
                        e.0,
                        10 + index as u128,
                        layer,
                        name.as_mut_ptr(),
                        std::slice::from_mut(&mut condition),
                    )?;
                }
                checked(FwpmTransactionCommit0(e.0), "завершение транзакции")
            })();
            if outcome.is_err() {
                FwpmTransactionAbort0(e.0);
            }
            FwpmFreeMemory0(&mut app_id as *mut _ as *mut _);
            if !xray_app_id.is_null() {FwpmFreeMemory0(&mut xray_app_id as *mut _ as *mut _);}
            if outcome.is_ok() && tun_ready {
                *self.owned_tun.lock().map_err(|_| "Не удалось записать идентификатор TUN")? = tun_instance;
            }
            outcome
        }
    }
}

unsafe fn permit(
    engine: HANDLE,
    id: u128,
    layer: GUID,
    name: *mut u16,
    conditions: &mut [FWPM_FILTER_CONDITION0],
) -> Result<(), String> {
    for condition in conditions.iter_mut() {
        condition.matchType = FWP_MATCH_EQUAL;
    }
    let mut filter: FWPM_FILTER0 = std::mem::zeroed();
    filter.filterKey = key(id);
    filter.displayData.name = name;
    filter.layerKey = layer;
    filter.subLayerKey = SESSION_SUBLAYER;
    filter.weight.r#type = FWP_UINT8;
    filter.weight.Anonymous.uint8 = 10;
    filter.action.r#type = FWP_ACTION_PERMIT;
    filter.numFilterConditions = conditions.len() as u32;
    filter.filterCondition = conditions.as_mut_ptr();
    checked(
        FwpmFilterAdd0(engine, &filter, ptr::null_mut(), ptr::null_mut()),
        "исключение локальной сети",
    )
}

fn dhcp_conditions(local: u16, remote: u16) -> [FWPM_FILTER_CONDITION0; 3] {
    // No destination restriction: both discovery broadcast and unicast lease
    // renewal use these ports. No blanket UDP permit is introduced.
    let mut conditions = unsafe { [std::mem::zeroed::<FWPM_FILTER_CONDITION0>(); 3] };
    conditions[0].fieldKey = FWPM_CONDITION_IP_PROTOCOL;
    conditions[0].conditionValue.r#type = FWP_UINT8;
    conditions[0].conditionValue.Anonymous.uint8 = 17;
    conditions[1].fieldKey = FWPM_CONDITION_IP_LOCAL_PORT;
    conditions[1].conditionValue.r#type = FWP_UINT16;
    conditions[1].conditionValue.Anonymous.uint16 = local;
    conditions[2].fieldKey = FWPM_CONDITION_IP_REMOTE_PORT;
    conditions[2].conditionValue.r#type = FWP_UINT16;
    conditions[2].conditionValue.Anonymous.uint16 = remote;
    conditions
}
#[cfg(test)]
mod tests {
    #[test]
    fn cleanup_never_reports_success_with_a_remaining_tun() {
        assert!(super::wait_for_tun_release_with(std::time::Duration::ZERO,||Some(42)).is_err());
        assert!(super::wait_for_tun_release_with(std::time::Duration::ZERO,||None).is_ok());
    }
    use super::*;
    #[test]
    fn dhcp_discovery_and_renewal_share_port_scoped_wfp_conditions() {
        for (local, remote) in [(68, 67), (546, 547)] {
            let conditions = dhcp_conditions(local, remote);
            assert_eq!(conditions.len(), 3);
            assert_eq!(conditions[0].fieldKey.data1, FWPM_CONDITION_IP_PROTOCOL.data1);
            assert_eq!(conditions[1].fieldKey.data1, FWPM_CONDITION_IP_LOCAL_PORT.data1);
            assert_eq!(conditions[2].fieldKey.data1, FWPM_CONDITION_IP_REMOTE_PORT.data1);
            unsafe {
                assert_eq!(conditions[0].conditionValue.Anonymous.uint8, 17);
                assert_eq!(conditions[1].conditionValue.Anonymous.uint16, local);
                assert_eq!(conditions[2].conditionValue.Anonymous.uint16, remote);
            }
        }
    }
    #[test]
    fn lan_exceptions_do_not_allow_public_or_fake_ip_destinations() {
        let allowed = |ip: &str| {
            let ip = ip.parse::<std::net::IpAddr>().unwrap();
            crate::lan_policy::PREFIXES.iter().any(|network| network.parse::<ipnet::IpNet>().unwrap().contains(&ip))
        };
        for ip in ["192.168.1.1", "10.1.2.3", "172.16.0.1", "172.31.255.254"] {
            assert!(allowed(ip), "{ip}");
        }
        for ip in [
            "1.1.1.1",
            "8.8.8.8",
            "172.15.255.255",
            "172.32.0.1",
            "198.19.0.1",
        ] {
            assert!(!allowed(ip), "{ip}");
        }
    }
}
