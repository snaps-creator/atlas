//! Session-owned WFP policy. Only the owned core, loopback and the TUN interface
//! can originate traffic. DIRECT exceptions are still evaluated by Mihomo.
//! Windows releases dynamic filters if the helper dies, restoring prior policy.
use std::{path::Path, ptr};
use windows_sys::{
    core::GUID,
    Win32::{
        Foundation::HANDLE,
        NetworkManagement::{
            IpHelper::ConvertInterfaceAliasToLuid, Ndis::NET_LUID_LH, WindowsFilteringPlatform::*,
        },
    },
};
const SUBLAYER: GUID = GUID::from_u128(0xe12a8041_d824_4776_978b_31a129691c00);
fn key(index: u128) -> GUID {
    GUID::from_u128(0xe12a8041_d824_4776_978b_31a129692000 + index)
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
    fn open() -> Result<Self, String> {
        Self::with_session(false)
    }
    fn with_session(dynamic: bool) -> Result<Self, String> {
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
unsafe fn erase(engine: HANDLE) -> Result<(), String> {
    for i in 0..8 {
        let r = FwpmFilterDeleteByKey0(engine, &key(i));
        if r != 0 && r != 0x80320003 {
            return Err(format!("Не удалось удалить фильтр Атласа: {r:#x}"));
        }
    }
    let r = FwpmSubLayerDeleteByKey0(engine, &SUBLAYER);
    if r != 0 && r != 0x80320007 {
        return Err(format!("Не удалось удалить слой Атласа: {r:#x}"));
    }
    Ok(())
}
pub fn clear() -> Result<(), String> {
    let e = Engine::open()?;
    unsafe {
        checked(FwpmTransactionBegin0(e.0, 0), "транзакция")?;
        if let Err(err) = erase(e.0) {
            FwpmTransactionAbort0(e.0);
            return Err(err);
        }
        checked(FwpmTransactionCommit0(e.0), "завершение транзакции")
    }
}
pub struct Guard(Engine);

/// A competing full-tunnel must be disconnected by its owner before Atlas connects.
/// Reading routes has no side effects; never delete another client's routes/filters.
pub fn check_competing_routes() -> Result<(), String> {
    use windows_sys::Win32::{
        NetworkManagement::IpHelper::{FreeMibTable, GetIpForwardTable2},
        Networking::WinSock::AF_UNSPEC,
    };
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
    pub fn prepare(_core: &Path) -> Result<Self, String> {
        clear()?; // Remove only Atlas-owned filters left by older releases.
                  // Do not block the entire host while the adapter has not even been created.
                  // The caller reports Connected only after install has committed the policy.
        Ok(Self(Engine::with_session(true)?))
    }
    pub fn install(&self, core: &Path) -> Result<(), String> {
        self.policy(core, true)
    }
    fn policy(&self, core: &Path, tun_ready: bool) -> Result<(), String> {
        let e = &self.0;
        unsafe {
            let mut luid: NET_LUID_LH = std::mem::zeroed();
            if tun_ready {
                checked(
                    ConvertInterfaceAliasToLuid(wide("Atlas-TUN").as_ptr(), &mut luid),
                    "интерфейс Atlas-TUN не найден",
                )?;
            }
            let mut app_id = ptr::null_mut();
            checked(
                FwpmGetAppIdFromFileName0(wide(&core.to_string_lossy()).as_ptr(), &mut app_id),
                "идентификатор ядра",
            )?;
            let outcome = (|| {
                checked(FwpmTransactionBegin0(e.0, 0), "транзакция")?;
                erase(e.0)?;
                let mut name = wide("Атлас — защита VPN");
                let mut sub: FWPM_SUBLAYER0 = std::mem::zeroed();
                sub.subLayerKey = SUBLAYER;
                sub.flags = 0;
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
                        filter.subLayerKey = SUBLAYER;
                        filter.flags = 0;
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
                checked(FwpmTransactionCommit0(e.0), "завершение транзакции")
            })();
            if outcome.is_err() {
                FwpmTransactionAbort0(e.0);
            }
            FwpmFreeMemory0(&mut app_id as *mut _ as *mut _);
            outcome
        }
    }
}
