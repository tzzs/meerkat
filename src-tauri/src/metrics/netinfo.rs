//! TCP ownership: which process holds which connection and which listening port.
//! Ordinary user rights are enough for the owner-PID tables.
//!
//! The IPv4 and IPv6 tables have different rows (24 and 56 bytes), and each is read with its own
//! layout. They used to share the IPv4 one, which walked the IPv6 table at the wrong stride and
//! read pids, ports and states out of the middle of addresses.

use std::collections::HashMap;
use std::net::{Ipv4Addr, Ipv6Addr};

use windows::Win32::NetworkManagement::IpHelper::{
    GetExtendedTcpTable, MIB_TCP6ROW_OWNER_PID, MIB_TCP6TABLE_OWNER_PID, MIB_TCPROW_OWNER_PID,
    MIB_TCPTABLE_OWNER_PID, TCP_TABLE_OWNER_PID_ALL,
};
use windows::Win32::Networking::WinSock::{AF_INET, AF_INET6};

const STATE_LISTEN: u32 = 2;

/// One listening socket.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Listener {
    pub port: u16,
    pub pid: u32,
    /// The address it is bound to: `0.0.0.0` and `::` accept from the network, `127.0.0.1` and
    /// `::1` only from this machine.
    pub address: String,
}

#[derive(Default)]
pub struct TcpOwnership {
    pub connections: HashMap<u32, usize>,
    pub listening: HashMap<u32, Vec<u16>>,
    pub total_connections: usize,
    pub listeners: Vec<Listener>,
}

/// One row of either table, reduced to what Pulse uses.
struct Row {
    pid: u32,
    state: u32,
    port: u16,
    address: String,
}

/// The port fields hold a 16-bit number in network byte order in their low half.
fn port_of(field: u32) -> u16 {
    u16::from_be(field as u16)
}

/// Fills `buffer` with one table, growing it until the table fits. Empty on any failure.
fn read_table(family: u32) -> Vec<u64> {
    for _ in 0..4 {
        let mut size = 0u32;
        unsafe {
            GetExtendedTcpTable(None, &mut size, true, family, TCP_TABLE_OWNER_PID_ALL, 0);
        }
        if size == 0 {
            return Vec::new();
        }
        // u64-backed so the rows, which start at an 8-byte boundary, are aligned.
        let mut buffer = vec![0u64; (size as usize + 512).div_ceil(8)];
        let mut filled = (buffer.len() * 8) as u32;
        let status = unsafe {
            GetExtendedTcpTable(Some(buffer.as_mut_ptr().cast()), &mut filled, true, family, TCP_TABLE_OWNER_PID_ALL, 0)
        };
        match status {
            0 => return buffer,
            // ERROR_INSUFFICIENT_BUFFER: connections opened between the two calls.
            122 => continue,
            _ => return Vec::new(),
        }
    }
    Vec::new()
}

fn rows_v4() -> Vec<Row> {
    let buffer = read_table(AF_INET.0 as u32);
    if buffer.is_empty() {
        return Vec::new();
    }
    let table = buffer.as_ptr().cast::<MIB_TCPTABLE_OWNER_PID>();
    let count = unsafe { (*table).dwNumEntries } as usize;
    let capacity = (buffer.len() * 8 - 8) / std::mem::size_of::<MIB_TCPROW_OWNER_PID>();
    let rows = unsafe { std::slice::from_raw_parts((*table).table.as_ptr(), count.min(capacity)) };
    rows.iter()
        .map(|row| Row {
            pid: row.dwOwningPid,
            state: row.dwState,
            port: port_of(row.dwLocalPort),
            // Stored in network byte order, so the bytes in memory are already the octets.
            address: Ipv4Addr::from(row.dwLocalAddr.to_ne_bytes()).to_string(),
        })
        .collect()
}

fn rows_v6() -> Vec<Row> {
    let buffer = read_table(AF_INET6.0 as u32);
    if buffer.is_empty() {
        return Vec::new();
    }
    let table = buffer.as_ptr().cast::<MIB_TCP6TABLE_OWNER_PID>();
    let count = unsafe { (*table).dwNumEntries } as usize;
    let capacity = (buffer.len() * 8 - 8) / std::mem::size_of::<MIB_TCP6ROW_OWNER_PID>();
    let rows = unsafe { std::slice::from_raw_parts((*table).table.as_ptr(), count.min(capacity)) };
    rows.iter()
        .map(|row| Row {
            pid: row.dwOwningPid,
            state: row.dwState,
            port: port_of(row.dwLocalPort),
            address: Ipv6Addr::from(row.ucLocalAddr).to_string(),
        })
        .collect()
}

pub fn collect() -> TcpOwnership {
    let mut owned = TcpOwnership::default();
    for row in rows_v4().into_iter().chain(rows_v6()) {
        if row.pid == 0 {
            continue;
        }
        if row.state == STATE_LISTEN {
            let entry = owned.listening.entry(row.pid).or_default();
            if !entry.contains(&row.port) {
                entry.push(row.port);
            }
            owned.listeners.push(Listener { port: row.port, pid: row.pid, address: row.address });
        } else {
            *owned.connections.entry(row.pid).or_insert(0) += 1;
            owned.total_connections += 1;
        }
    }
    for ports in owned.listening.values_mut() {
        ports.sort_unstable();
        ports.dedup();
    }
    owned.listeners.sort_by(|a, b| a.port.cmp(&b.port).then_with(|| a.address.cmp(&b.address)));
    owned.listeners.dedup();
    owned
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ports_are_read_in_network_byte_order() {
        // 1420 is 0x058C; on the wire, and so in the low half of the field, it is 8C 05.
        assert_eq!(port_of(u32::from(u16::from_ne_bytes([0x05, 0x8C]))), 1420);
    }

    /// Both tables, read with their own layouts, find sockets this test opens itself.
    #[test]
    fn the_live_tables_find_our_own_listeners_on_both_families() {
        let v4 = std::net::TcpListener::bind("127.0.0.1:0").expect("an IPv4 loopback port");
        let v6 = std::net::TcpListener::bind("[::1]:0").ok();
        let me = std::process::id();
        let owned = collect();
        let mine: Vec<&Listener> = owned.listeners.iter().filter(|l| l.pid == me).collect();
        let port4 = v4.local_addr().unwrap().port();
        assert!(
            mine.iter().any(|l| l.port == port4 && l.address == "127.0.0.1"),
            "IPv4 listener {port4} not found in {mine:?}"
        );
        if let Some(v6) = v6 {
            let port6 = v6.local_addr().unwrap().port();
            assert!(mine.iter().any(|l| l.port == port6 && l.address == "::1"), "IPv6 listener {port6} not found in {mine:?}");
        }
        assert!(owned.listening.get(&me).is_some_and(|ports| ports.contains(&port4)));
    }
}
