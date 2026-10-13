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
            IpHelper::{ConvertInterfaceAliasToLuid, ConvertInterfaceLuidToGuid,
                DeleteIpForwardEntry2, GetIfEntry2Ex, MibIfEntryNormalWithoutStatistics,
                MIB_IF_ROW2},
            Ndis::{IfOperStatusDown, IfOperStatusLowerLayerDown, IfOperStatusNotPresent,
                NET_LUID_LH}, WindowsFilteringPlatform::*,
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
const OWNED_TUN_KEY: &str = r"SOFTWARE\AtlasVPN\NetworkOwnership";
fn recorded_tun() -> Option<(u64, u128)> {
    use winreg::{RegKey, enums::{HKEY_LOCAL_MACHINE, KEY_QUERY_VALUE}};
    let key = RegKey::predef(HKEY_LOCAL_MACHINE).open_subkey_with_flags(OWNED_TUN_KEY, KEY_QUERY_VALUE).ok()?;
    let value: String = key.get_value("TunIdentity").ok()?;
    let (luid, guid) = value.split_once(':')?;
    Some((luid.parse().ok()?, u128::from_str_radix(guid, 16).ok()?))
}
fn record_owned_tun(identity: (u64, u128)) -> Result<(), String> {
    use winreg::{RegKey, enums::HKEY_LOCAL_MACHINE};
    if recorded_tun() == Some(identity) { return Ok(()); }
    // Written by the privileged service; ordinary desktop clients only read.
    // One value prevents a reader observing half of an identity update.
    let (key, _) = RegKey::predef(HKEY_LOCAL_MACHINE).create_subkey(OWNED_TUN_KEY)
        .map_err(|e| format!("Запись владения Atlas-TUN: {e}"))?;
    key.set_value("TunIdentity", &format!("{}:{:032x}",identity.0,identity.1))
        .map_err(|e| format!("Запись идентификатора Atlas-TUN: {e}"))
}
fn owns_current_tun(luid: u64) -> bool {
    let Some((recorded_luid, recorded_guid)) = recorded_tun() else { return false; };
    if luid != recorded_luid { return false; }
    unsafe {
        let mut id: NET_LUID_LH = std::mem::zeroed();
        id.Value = luid;
        let mut guid: GUID = std::mem::zeroed();
        ConvertInterfaceLuidToGuid(&id, &mut guid) == 0 && guid_value(guid) == recorded_guid
    }
}

#[derive(Clone, Debug, Default)]
struct TunProbe {
    alias_luid: Option<u64>,
    query_code: Option<u32>,
    oper_status: Option<i32>,
    interface_type: Option<u32>,
    description: Option<String>,
    verified_wintun: bool,
    driver_observed: Option<bool>,
    route_count: Option<usize>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize)]
pub(crate) enum TunState {
    Missing, Available, ActiveOwnedByAtlas, ActiveForeign,
    DisabledReusable, CleanupRequired, InvalidDriver, Unknown,
}
fn is_down(probe: &TunProbe) -> bool {
    probe.query_code == Some(0) && probe.oper_status.is_some_and(|s|
        s == IfOperStatusDown || s == IfOperStatusLowerLayerDown || s == IfOperStatusNotPresent)
}
fn classify_tun(probe: &TunProbe, owned: Option<u64>) -> TunState {
    if probe.alias_luid.is_none() {
        return if probe.query_code.is_none() || matches!(probe.query_code, Some(2 | 1168)) {
            TunState::Missing
        } else { TunState::Unknown };
    }
    if matches!(probe.query_code, Some(2 | 1168)) {
        return if probe.route_count == Some(0) { TunState::Missing } else { TunState::CleanupRequired };
    }
    if probe.query_code != Some(0) { return TunState::Unknown; }
    if probe.driver_observed == Some(false) {
        return if probe.oper_status == Some(windows_sys::Win32::NetworkManagement::Ndis::IfOperStatusUp) {
            TunState::ActiveForeign
        } else { TunState::InvalidDriver };
    }
    if is_down(probe) {
        if probe.route_count.is_some_and(|n| n > 0) { return TunState::CleanupRequired; }
        if reusable_wintun(probe) { return TunState::DisabledReusable; }
        // A withdrawn driver key is not proof of an active owner. There is
        // nothing to clean when Windows confirms DOWN and no routes remain.
        if probe.route_count == Some(0) { return TunState::Available; }
        return TunState::Unknown;
    }
    if probe.oper_status == Some(windows_sys::Win32::NetworkManagement::Ndis::IfOperStatusUp) {
        return if owned == probe.alias_luid { TunState::ActiveOwnedByAtlas }
            else { TunState::Unknown };
    }
    TunState::Unknown
}
fn baseline_ready(probe: &TunProbe) -> bool {
    matches!(classify_tun(probe, None), TunState::Missing | TunState::Available | TunState::DisabledReusable)
        && (probe.alias_luid.is_none() || probe.route_count == Some(0))
}

// The interface description is chosen by the application (Mihomo uses
// "Meta Tunnel"). Verify the actual driver through the adapter's GUID instead.
fn verified_wintun_driver(luid: &NET_LUID_LH) -> Option<bool> {
    use winreg::{RegKey,enums::{HKEY_LOCAL_MACHINE, KEY_ENUMERATE_SUB_KEYS, KEY_QUERY_VALUE}};
    let mut guid: GUID=unsafe {std::mem::zeroed()};
    if unsafe {ConvertInterfaceLuidToGuid(luid,&mut guid)}!=0 {return None;}
    let id=format!("{{{:08x}-{:04x}-{:04x}-{:02x}{:02x}-{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}}}",
        guid.data1,guid.data2,guid.data3,guid.data4[0],guid.data4[1],guid.data4[2],guid.data4[3],guid.data4[4],guid.data4[5],guid.data4[6],guid.data4[7]);
    let machine=RegKey::predef(HKEY_LOCAL_MACHINE);
    // PnP device instance IDs are not network-interface GUIDs. In particular,
    // assuming SWD\Wintun\{NetCfgInstanceId} misclassifies an already DOWN
    // adapter as active. Resolve the driver's authoritative network class key,
    // as Wintun itself does, instead of reconstructing a PnP instance path.
    let observed = machine.open_subkey_with_flags(
        r"SYSTEM\CurrentControlSet\Control\Class\{4D36E972-E325-11CE-BFC1-08002BE10318}",
        KEY_ENUMERATE_SUB_KEYS).ok().and_then(|class| {
        class.enum_keys().filter_map(Result::ok).find_map(|name| {
            // Querying values does not require enumeration/notification access
            // to each device key (KEY_READ requests those additional rights).
            let key=class.open_subkey_with_flags(name, KEY_QUERY_VALUE).ok()?;
            let instance=key.get_value::<String,_>("NetCfgInstanceId").ok()?;
            if !id.eq_ignore_ascii_case(&instance) { return None; }
            let component=key.get_value::<String,_>("ComponentId").ok()?;
            Some(wintun_class_matches(&id,&instance,&component))
        })
    }).or_else(|| {
        // Resolve the PnP instance from Windows' GUID mapping, not by guessing
        // its path. This also works when enumerating class keys is restricted.
        let connection=machine.open_subkey_with_flags(format!(
            r"SYSTEM\CurrentControlSet\Control\Network\{{4D36E972-E325-11CE-BFC1-08002BE10318}}\{id}\Connection"),
            KEY_QUERY_VALUE).ok()?;
        let instance=connection.get_value::<String,_>("PnpInstanceID").ok()?;
        if instance.is_empty() { return None; }
        let device=machine.open_subkey_with_flags(format!(r"SYSTEM\CurrentControlSet\Enum\{instance}"),KEY_QUERY_VALUE).ok()?;
        Some(wintun_device_matches(&device.get_value::<String,_>("ClassGUID").ok()?,
            &device.get_value::<String,_>("Service").ok()?,
            &device.get_value::<Vec<String>,_>("HardwareID").ok()?))
    });
    // No process-local positive cache: desktop and service use the same
    // current evidence, including an explicit Unknown when a key disappears.
    resolve_driver_evidence(observed, owns_current_tun(unsafe { luid.Value }))
}
fn resolve_driver_evidence(observed: Option<bool>, owned_instance: bool) -> Option<bool> {
    observed.or_else(|| owned_instance.then_some(true))
}

fn tun_route_count(luid: u64) -> Option<usize> {
    use windows_sys::Win32::{NetworkManagement::IpHelper::{GetIpForwardTable2, FreeMibTable}, Networking::WinSock::AF_UNSPEC};
    unsafe {
        let mut table = ptr::null_mut();
        if GetIpForwardTable2(AF_UNSPEC, &mut table) != 0 { return None; }
        let rows = std::slice::from_raw_parts((*table).Table.as_ptr(), (*table).NumEntries as usize);
        let count = rows.iter().filter(|r| r.InterfaceLuid.Value == luid).count();
        FreeMibTable(table.cast());
        Some(count)
    }
}

fn wintun_class_matches(expected: &str, instance: &str, component: &str) -> bool {
    !expected.is_empty() && expected.eq_ignore_ascii_case(instance)
        && component.eq_ignore_ascii_case("wintun")
}

fn wintun_device_matches(class: &str, service: &str, hardware: &[String]) -> bool {
    class.eq_ignore_ascii_case("{4D36E972-E325-11CE-BFC1-08002BE10318}")
        && service.eq_ignore_ascii_case("wintun")
        && hardware.iter().any(|id|id.eq_ignore_ascii_case("wintun"))
}

fn tun_probe() -> TunProbe {
    unsafe {
        let mut tun: NET_LUID_LH = std::mem::zeroed();
        if ConvertInterfaceAliasToLuid(wide("Atlas-TUN").as_ptr(), &mut tun) != 0 {
            use windows_sys::Win32::NetworkManagement::IpHelper::{GetIfTable2Ex, FreeMibTable, MibIfTableNormal};
            let mut table = ptr::null_mut();
            let status = GetIfTable2Ex(MibIfTableNormal, &mut table);
            if status != 0 { return TunProbe { query_code: Some(status), ..Default::default() }; }
            let rows = std::slice::from_raw_parts((*table).Table.as_ptr(), (*table).NumEntries as usize);
            let found = rows.iter().find(|row| String::from_utf16_lossy(&row.Alias).trim_matches('\0') == "Atlas-TUN")
                .map(|row| row.InterfaceLuid);
            FreeMibTable(table.cast());
            let Some(found) = found else { return TunProbe::default(); };
            tun = found;
        }
        let mut row: MIB_IF_ROW2 = std::mem::zeroed();
        row.InterfaceLuid = tun;
        // Alias lookup can outlive a Wintun adapter after an abrupt reboot.
        // Ask IP Helper whether the interface itself still exists. This variant
        // avoids querying driver statistics while Windows is recovering.
        let status = GetIfEntry2Ex(MibIfEntryNormalWithoutStatistics, &mut row);
        let driver = if status == 0 { verified_wintun_driver(&tun) } else { None };
        TunProbe {
            alias_luid: Some(tun.Value),
            query_code: Some(status),
            oper_status: (status == 0).then_some(row.OperStatus),
            interface_type: (status == 0).then_some(row.Type),
            description: (status == 0).then(|| String::from_utf16_lossy(&row.Description)
                .trim_matches('\0').to_owned()),
            verified_wintun: driver == Some(true),
            driver_observed: driver,
            route_count: tun_route_count(tun.Value),
        }
    }
}

fn present_tun(alias: Option<u64>, query_code: Option<u32>) -> Option<u64> {
    match query_code {
        // ERROR_FILE_NOT_FOUND / ERROR_NOT_FOUND: stale alias, no interface.
        Some(2 | 1168) => None,
        // Other errors are not proof of absence: retain the collision guard.
        _ => alias,
    }
}

fn reusable_wintun(probe: &TunProbe) -> bool {
    probe.query_code == Some(0)
        && probe.interface_type == Some(windows_sys::Win32::NetworkManagement::IpHelper::IF_TYPE_PROP_VIRTUAL)
        && probe.verified_wintun
        && probe.oper_status.is_some_and(|status| status == IfOperStatusDown
            || status == IfOperStatusLowerLayerDown || status == IfOperStatusNotPresent)
}

fn active_tun(probe: &TunProbe) -> Option<u64> {
    let alias = present_tun(probe.alias_luid, probe.query_code)?;
    if reusable_wintun(probe) || baseline_ready(probe) { None } else { Some(alias) }
}

pub fn tun_identity() -> Option<u64> {
    active_tun(&tun_probe())
}

pub(crate) fn tun_diagnostic() -> serde_json::Value {
    let probe = tun_probe();
    diagnostic_probe(&probe, None)
}
fn diagnostic_probe(probe: &TunProbe, owned: Option<u64>) -> serde_json::Value {
    serde_json::json!({
        "aliasLuid": probe.alias_luid,
        "interfaceQueryCode": probe.query_code,
        "presentLuid": present_tun(probe.alias_luid, probe.query_code),
        "operStatus": probe.oper_status,
        "interfaceType": probe.interface_type,
        "description": probe.description,
        "verifiedWintunDriver": probe.verified_wintun,
        "driverEvidence": probe.driver_observed,
        "ownershipRecorded": probe.alias_luid.is_some_and(owns_current_tun),
        "liveOwnershipConfirmed": owned.is_some() && owned == probe.alias_luid,
        "classificationMeaning": "Marker ownership is historical evidence; Unknown requires live session ownership confirmation and does not mean a routing failure",
        "state": classify_tun(probe, owned),
        "adapterName": "Atlas-TUN",
        "routeCount": probe.route_count,
        "baselineReady": baseline_ready(&probe),
        "adapterEnabled": probe.oper_status.map(|s| s == windows_sys::Win32::NetworkManagement::Ndis::IfOperStatusUp),
        "reusableWintun": reusable_wintun(&probe),
        "activeLuid": if probe.query_code == Some(0)
            && probe.oper_status == Some(windows_sys::Win32::NetworkManagement::Ndis::IfOperStatusUp) {
                probe.alias_luid
            } else { None },
        "blockingLuid": active_tun(&probe),
        "meaning": "A verified, down Wintun adapter may persist after its process exits and can be reused",
    })
}

/// Remove only route entries tied to Atlas' exact, verified Wintun adapter,
/// and only while Windows reports that adapter down. This repairs split-default
/// routes left by a hard core/service crash without touching other VPN routes.
pub(crate) fn clear_routes_for_reusable_tun() -> Result<usize, String> {
    use windows_sys::Win32::{
        Foundation::{ERROR_FILE_NOT_FOUND, ERROR_NOT_FOUND},
        NetworkManagement::IpHelper::{FreeMibTable, GetIpForwardTable2, MIB_IPFORWARD_ROW2},
        Networking::WinSock::AF_UNSPEC,
    };
    let probe = tun_probe();
    let Some(luid) = probe.alias_luid else {
        return if baseline_ready(&probe) { Ok(0) } else { Err("Не удалось проверить наличие Atlas-TUN".into()) };
    };
    if baseline_ready(&probe) { return Ok(0); }
    let stale_alias = matches!(probe.query_code, Some(2 | 1168));
    if !owns_current_tun(luid) {
        return Err("Оставшиеся маршруты Atlas-TUN не имеют подтверждённой записи владения; чужие маршруты не изменены".into());
    }
    if !stale_alias && !reusable_wintun(&probe) {
        return Err(format!("Очистка Atlas-TUN не подтверждена: состояние {:?}, драйвер {:?}, маршруты {:?} (LUID {luid})",
            classify_tun(&probe, None), probe.driver_observed, probe.route_count));
    }
    unsafe {
        let mut table = ptr::null_mut();
        checked(GetIpForwardTable2(AF_UNSPEC, &mut table), "чтение маршрутов Atlas-TUN")?;
        let rows = std::slice::from_raw_parts((*table).Table.as_ptr(), (*table).NumEntries as usize);
        let stale: Vec<MIB_IPFORWARD_ROW2> = rows.iter()
            .filter(|route| route.InterfaceLuid.Value == luid)
            .copied().collect();
        FreeMibTable(table.cast());
        let mut removed = 0;
        for route in stale {
            let current = tun_probe();
            if current.alias_luid != Some(luid) || !owns_current_tun(luid)
                || (!matches!(current.query_code, Some(2 | 1168)) && !reusable_wintun(&current)) {
                return Err("Состояние Atlas-TUN изменилось во время очистки; удаление маршрутов остановлено".into());
            }
            let status = DeleteIpForwardEntry2(&route);
            if status == 0 { removed += 1; }
            else if status != ERROR_NOT_FOUND && status != ERROR_FILE_NOT_FOUND {
                return Err(format!("Не удалось удалить устаревший маршрут Atlas-TUN: код Windows {status:#x}"));
            }
        }
        Ok(removed)
    }
}

pub(crate) fn wait_for_tun_release(timeout: std::time::Duration) -> Result<(), String> {
    wait_for_tun_release_with(timeout, tun_identity)
}
pub(crate) fn wait_for_clean_baseline(timeout: std::time::Duration) -> Result<(), String> {
    let deadline = std::time::Instant::now() + timeout;
    loop {
        let probe = tun_probe();
        if baseline_ready(&probe) { return Ok(()); }
        if std::time::Instant::now() >= deadline {
            return Err(format!("Сеть Atlas требует восстановления: {:?}; маршруты {:?}",
                classify_tun(&probe, None), probe.route_count));
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
}
fn wait_for_tun_release_with(timeout: std::time::Duration, identity: impl Fn() -> Option<u64>) -> Result<(), String> {
    let deadline = std::time::Instant::now() + timeout;
    while identity().is_some() {
        if std::time::Instant::now() >= deadline {
            return Err("Освобождение Atlas-TUN после остановки службы не подтверждено; неизвестный интерфейс не изменён".into());
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

fn competing_capture_route(prefix_length: u8, loopback: bool, atlas: bool, interface_type: Option<u32>) -> bool {
    if loopback || atlas { return false; }
    // A number of Clash/Mihomo clients install one virtual 0/0 route rather
    // than the traditional pair of /1 routes. A physical 0/0 is the normal
    // uplink and must remain allowed; a foreign virtual 0/0 captures the same
    // traffic as Atlas and makes the two TUN engines race for Windows traffic.
    prefix_length == 1 || (prefix_length == 0
        && interface_type == Some(windows_sys::Win32::NetworkManagement::IpHelper::IF_TYPE_PROP_VIRTUAL))
}

fn competing_capture_routes() -> Result<Vec<(u64,u8,Option<u32>)>, String> {
    use windows_sys::Win32::{
        NetworkManagement::IpHelper::{FreeMibTable, GetIpForwardTable2},
        Networking::WinSock::AF_UNSPEC,
    };
    unsafe {
        let mut table = ptr::null_mut();
        checked(GetIpForwardTable2(AF_UNSPEC, &mut table), "проверка маршрутов перед подключением")?;
        let rows = std::slice::from_raw_parts((*table).Table.as_ptr(), (*table).NumEntries as usize);
        let atlas_luid = tun_probe().alias_luid;
        let mut conflicts = Vec::new();
        for route in rows {
            let mut interface: MIB_IF_ROW2 = std::mem::zeroed();
            interface.InterfaceLuid = route.InterfaceLuid;
            let interface_type = (GetIfEntry2Ex(MibIfEntryNormalWithoutStatistics, &mut interface) == 0)
                .then_some(interface.Type);
            if competing_capture_route(route.DestinationPrefix.PrefixLength, route.Loopback != 0,
                Some(route.InterfaceLuid.Value) == atlas_luid, interface_type) {
                conflicts.push((route.InterfaceLuid.Value, route.DestinationPrefix.PrefixLength, interface_type));
            }
        }
        FreeMibTable(table.cast());
        conflicts.sort_unstable();
        conflicts.dedup();
        Ok(conflicts)
    }
}

pub(crate) fn competing_route_diagnostic() -> serde_json::Value {
    match competing_capture_routes() {
        Ok(routes) => serde_json::json!({"detected":!routes.is_empty(),"routes":routes.into_iter().map(|(luid,prefix,kind)|
            serde_json::json!({"interfaceLuid":luid,"prefixLength":prefix,"interfaceType":kind})).collect::<Vec<_>>(),
            "meaning":"A foreign virtual default route belongs to another full-tunnel VPN and conflicts with Atlas TUN"}),
        Err(error) => serde_json::json!({"detected":null,"error":error}),
    }
}

/// A competing full-tunnel must be disconnected by its owner before Atlas connects.
/// Reading routes has no side effects; never delete another client's routes/filters.
pub fn check_competing_routes() -> Result<(), String> {
    clear_routes_for_reusable_tun()?;
    if tun_identity().is_some() {
        return Err("Интерфейс Atlas-TUN активен или его тип нельзя подтвердить; запуск остановлен без изменения адаптера".into());
    }
    if !competing_capture_routes()?.is_empty() {
        return Err("Обнаружен активный туннель другого VPN. Отключите его перед подключением Atlas. Сетевые настройки не изменены.".into());
    }
    Ok(())
}

impl Guard {
    pub(crate) fn diagnostic(&self) -> serde_json::Value {
        let probe = tun_probe();
        let owned = self.owned_tun.lock().ok().and_then(|slot| *slot).and_then(|(luid, guid)| {
            if probe.alias_luid != Some(luid) { return None; }
            unsafe {
                let mut identity: NET_LUID_LH = std::mem::zeroed();
                identity.Value = luid;
                let mut current: GUID = std::mem::zeroed();
                (ConvertInterfaceLuidToGuid(&identity, &mut current) == 0 && guid_value(current) == guid).then_some(luid)
            }
        });
        diagnostic_probe(&probe, owned)
    }
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
            return Err("Освобождение Atlas-TUN не подтверждено; восстановление остановлено".into());
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
                if let Some(identity) = tun_instance {
                    let probe = tun_probe();
                    if !probe.verified_wintun { return Err("Драйвер созданного Atlas-TUN не подтверждён".into()); }
                    record_owned_tun(identity)?;
                }
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
    fn shared_ownership_evidence_never_overrides_a_conflicting_driver() {
        assert_eq!(super::resolve_driver_evidence(None, true), Some(true));
        assert_eq!(super::resolve_driver_evidence(None, false), None);
        assert_eq!(super::resolve_driver_evidence(Some(false), true), Some(false));
        assert_eq!(super::resolve_driver_evidence(Some(true), false), Some(true));
    }
    #[test]
    fn withdrawn_driver_key_with_no_routes_is_available_without_claiming_verified_driver() {
        let probe = super::TunProbe { alias_luid: Some(42), query_code: Some(0),
            oper_status: Some(super::IfOperStatusDown), route_count: Some(0), ..Default::default() };
        assert_eq!(super::classify_tun(&probe, None), super::TunState::Available);
        assert!(super::baseline_ready(&probe));
        assert!(!super::reusable_wintun(&probe));
        assert_eq!(super::active_tun(&probe), None);
    }
    #[test]
    fn retry_recomputes_cleanup_state_and_is_idempotent() {
        let mut probe = super::TunProbe { alias_luid: Some(42), query_code: Some(0),
            oper_status: Some(super::IfOperStatusDown), verified_wintun: true,
            interface_type: Some(windows_sys::Win32::NetworkManagement::IpHelper::IF_TYPE_PROP_VIRTUAL),
            route_count: Some(2), ..Default::default() };
        assert!(!super::baseline_ready(&probe));
        assert_eq!(super::classify_tun(&probe,None), super::TunState::CleanupRequired);
        probe.route_count = Some(0);
        for _ in 0..2 {
            assert!(super::baseline_ready(&probe));
            assert_eq!(super::classify_tun(&probe,None), super::TunState::DisabledReusable);
            assert_eq!(super::active_tun(&probe), None);
        }
    }
    #[test]
    fn driver_identity_uses_network_guid_not_pnp_instance_shape() {
        let id="{01234567-89ab-cdef-0123-456789abcdef}";
        assert!(super::wintun_class_matches(id,&id.to_uppercase(),"Wintun"));
        assert!(!super::wintun_class_matches(id,id,"tap0901"));
        assert!(!super::wintun_class_matches(id,"{different-adapter}","wintun"));
        assert!(!super::wintun_class_matches(id,"","wintun"));
        assert!(!super::wintun_class_matches(id,id,""));
        let net_class="{4d36e972-e325-11ce-bfc1-08002be10318}";
        assert!(super::wintun_device_matches(net_class,"Wintun",&["Wintun".into()]));
        assert!(!super::wintun_device_matches(net_class,"tap0901",&["Wintun".into()]));
        assert!(!super::wintun_device_matches(net_class,"Wintun",&["Meta Tunnel".into()]));
        assert!(!super::wintun_device_matches("unknown","Wintun",&["Wintun".into()]));
    }
    #[test]
    fn competing_vpn_detection_includes_virtual_default_routes() {
        use windows_sys::Win32::NetworkManagement::IpHelper::{IF_TYPE_ETHERNET_CSMACD, IF_TYPE_PROP_VIRTUAL};
        assert!(super::competing_capture_route(1,false,false,Some(IF_TYPE_ETHERNET_CSMACD)));
        assert!(super::competing_capture_route(0,false,false,Some(IF_TYPE_PROP_VIRTUAL)));
        assert!(!super::competing_capture_route(0,false,false,Some(IF_TYPE_ETHERNET_CSMACD)));
        assert!(!super::competing_capture_route(0,false,true,Some(IF_TYPE_PROP_VIRTUAL)));
        assert!(!super::competing_capture_route(24,false,false,Some(IF_TYPE_PROP_VIRTUAL)));
    }
    #[test]
    fn cleanup_never_reports_success_with_a_remaining_tun() {
        assert!(super::wait_for_tun_release_with(std::time::Duration::ZERO,||Some(42)).is_err());
        assert!(super::wait_for_tun_release_with(std::time::Duration::ZERO,||None).is_ok());
    }

    #[test]
    fn stale_alias_is_not_a_live_tun_but_unknown_query_failure_stays_blocked() {
        assert_eq!(super::present_tun(Some(42), Some(2)), None);
        assert_eq!(super::present_tun(Some(42), Some(1168)), None);
        assert_eq!(super::present_tun(Some(42), Some(0)), Some(42));
        assert_eq!(super::present_tun(Some(42), Some(87)), Some(42));
    }
    #[test]
    fn down_verified_wintun_is_reusable_but_active_or_unknown_interfaces_are_not() {
        let mut probe = super::TunProbe { alias_luid: Some(42), query_code: Some(0),
            oper_status: Some(super::IfOperStatusDown),
            interface_type: Some(windows_sys::Win32::NetworkManagement::IpHelper::IF_TYPE_PROP_VIRTUAL),
            description: Some("Meta Tunnel".into()), verified_wintun: true, ..Default::default() };
        assert!(super::reusable_wintun(&probe));
        assert_eq!(super::active_tun(&probe), None);
        probe.oper_status = Some(windows_sys::Win32::NetworkManagement::Ndis::IfOperStatusUp);
        assert!(!super::reusable_wintun(&probe));
        assert_eq!(super::active_tun(&probe), Some(42));
        probe.oper_status = Some(super::IfOperStatusDown);
        probe.description = Some("Third-party virtual adapter".into());
        probe.verified_wintun=false;
        assert_eq!(super::active_tun(&probe), Some(42));
        probe.description = Some("Wintun Userspace Tunnel".into());
        assert_eq!(super::active_tun(&probe), Some(42),"A display name is not proof of driver identity");
        probe.verified_wintun=true;
        probe.query_code = Some(87);
        assert_eq!(super::active_tun(&probe), Some(42));
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
