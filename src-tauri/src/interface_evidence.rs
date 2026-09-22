//! Read-only native counters: no shell, packet capture, or policy changes.
use serde_json::{json,Value};
use std::collections::HashMap;
#[derive(Default)]
pub(crate) struct Counters { previous: HashMap<u64,[u64;4]> }
fn delta(previous: Option<[u64;4]>, current: [u64;4]) -> Option<[u64;4]> {
    let old = previous?;
    if current.iter().zip(old).any(|(a,b)|*a < b) { return None; }
    Some(std::array::from_fn(|i|current[i]-old[i]))
}
impl Counters {
    pub fn sample(&mut self) -> Value {
        use windows_sys::Win32::NetworkManagement::IpHelper::{GetIfTable2,FreeMibTable};
        unsafe {
            let mut table=std::ptr::null_mut();
            let status=GetIfTable2(&mut table);
            if status != 0 { return json!({"error":status}); }
            let rows=std::slice::from_raw_parts((*table).Table.as_ptr(),(*table).NumEntries as usize);
            let mut next=HashMap::new();
            let values: Vec<_>=rows.iter().filter(|r|r.OperStatus == 1).map(|r| {
                let id=r.InterfaceLuid.Value;
                let current=[r.InErrors,r.OutErrors,r.InDiscards,r.OutDiscards];
                let changes=delta(self.previous.get(&id).copied(),current);
                next.insert(id,current);
                let alias=String::from_utf16_lossy(&r.Alias[..r.Alias.iter().position(|c|*c==0).unwrap_or(r.Alias.len())]);
                json!({"luid":id,"index":r.InterfaceIndex,"name":alias,"mtu":r.Mtu,
                    "receivedBytes":r.InOctets,"sentBytes":r.OutOctets,"counters":current,"delta":changes,
                    "counterOrder":["receiveErrors","sendErrors","receiveDiscards","sendDiscards"]})
            }).collect();
            FreeMibTable(table.cast());self.previous=next;
            json!({"at":crate::model::now(),"interfaces":values,"scope":"Null delta means first observation or counter reset. Counter increments do not identify a packet or prove a driver defect."})
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn historical_errors_and_counter_resets_are_not_new_packet_loss() {
        assert_eq!(delta(None,[166,0,0,0]),None);
        assert_eq!(delta(Some([166,0,0,0]),[166,0,0,0]),Some([0,0,0,0]));
        assert_eq!(delta(Some([166,0,0,0]),[169,0,0,0]),Some([3,0,0,0]));
        assert_eq!(delta(Some([166,0,0,0]),[0,0,0,0]),None);
    }
}
