//! TCP ownership: which process holds which connection and which listening port.
//! Ordinary user rights are enough for the owner-PID tables.

use std::collections::HashMap;
use windows::Win32::NetworkManagement::IpHelper::{
    GetExtendedTcpTable, TCP_TABLE_OWNER_PID_ALL,
};
use windows::Win32::NetworkManagement::IpHelper::MIB_TCPTABLE_OWNER_PID;
use windows::Win32::Networking::WinSock::{AF_INET, AF_INET6};

const STATE_LISTEN: u32 = 2;

#[derive(Default)]
pub struct TcpOwnership {
    pub connections: HashMap<u32, usize>,
    pub listening: HashMap<u32, Vec<u16>>,
    pub total_connections: usize,
}

fn table(af: u32) -> Vec<(u32, u32, u16, bool)> {
    let mut size = 0u32;
    unsafe {
        GetExtendedTcpTable(None, &mut size, true, af, TCP_TABLE_OWNER_PID_ALL, 0);
    }
    if size == 0 {
        return Vec::new();
    }
    let mut buffer = vec![0u8; size as usize + 64];
    let mut filled = size;
    let status = unsafe {
        GetExtendedTcpTable(
            Some(buffer.as_mut_ptr().cast()),
            &mut filled,
            true,
            af,
            TCP_TABLE_OWNER_PID_ALL,
            0,
        )
    };
    if status != 0 {
        return Vec::new();
    }
    let base = buffer.as_ptr().cast::<MIB_TCPTABLE_OWNER_PID>();
    let count = unsafe { (*base).dwNumEntries } as usize;
    let mut out = Vec::with_capacity(count);
    unsafe {
        let rows = (*base).table.as_ptr();
        for i in 0..count {
            let row = &*rows.add(i);
            // The port sits in the low 16 bits, in network byte order.
            let port = u16::from_be(row.dwLocalPort as u16);
            out.push((row.dwOwningPid, row.dwState, port, row.dwLocalAddr == 0));
        }
    }
    out
}

pub fn collect() -> TcpOwnership {
    let mut owned = TcpOwnership::default();
    for af in [AF_INET.0 as u32, AF_INET6.0 as u32] {
        for (pid, state, port, _) in table(af) {
            if pid == 0 {
                continue;
            }
            if state == STATE_LISTEN {
                let entry = owned.listening.entry(pid).or_default();
                if !entry.contains(&port) {
                    entry.push(port);
                }
            } else {
                *owned.connections.entry(pid).or_insert(0) += 1;
                owned.total_connections += 1;
            }
        }
    }
    for ports in owned.listening.values_mut() {
        ports.sort_unstable();
        ports.dedup();
    }
    owned
}
