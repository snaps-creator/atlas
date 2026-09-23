//! IPv4 directed broadcasts on local interfaces, never a sweep of address ranges.
use std::net::Ipv4Addr;
pub const PORT:u16=17944;
pub fn targets()->Vec<(Ipv4Addr,Ipv4Addr)> {
    use windows_sys::Win32::NetworkManagement::IpHelper::{GetIpAddrTable,MIB_IPADDRTABLE};
    unsafe {
        let mut size=0;
        GetIpAddrTable(std::ptr::null_mut(),&mut size,0);
        if size==0 || size>1024*1024 { return vec![]; }
        let mut buffer=vec![0u64;(size as usize+7)/8];
        let table=buffer.as_mut_ptr().cast::<MIB_IPADDRTABLE>();
        if GetIpAddrTable(table,&mut size,0)!=0 { return vec![]; }
        let rows=std::slice::from_raw_parts((*table).table.as_ptr(),(*table).dwNumEntries as usize);
        rows.iter().filter_map(|row| {
            let ip=Ipv4Addr::from(row.dwAddr.to_ne_bytes());
            let mask=u32::from_be_bytes(row.dwMask.to_ne_bytes());
            broadcast(ip,mask).map(|destination|(ip,destination))
        }).take(16).collect()
    }
}
fn broadcast(ip:Ipv4Addr,mask:u32)->Option<Ipv4Addr> {
    if !(ip.is_private() || ip.is_link_local()) || mask.count_ones()<8 || mask.count_ones()>30 || (!mask).wrapping_add(1)&!mask != 0 {return None;}
    Some(Ipv4Addr::from(u32::from(ip)|!mask))
}
#[cfg(test)] mod tests {
    use super::*;
    #[test] fn broadcasts_follow_real_prefix_not_assumed_slash24() {
        assert_eq!(broadcast(Ipv4Addr::new(192,168,2,20),0xfffffe00),Some(Ipv4Addr::new(192,168,3,255)));
        assert!(broadcast(Ipv4Addr::new(198,19,0,1),0xffff0000).is_none());
        assert!(broadcast(Ipv4Addr::new(8,8,8,8),0xffffff00).is_none());
        assert!(broadcast(Ipv4Addr::new(192,168,1,2),0xffffffff).is_none());
    }
}
