//! What the WSL virtual machine's memory is made of.
//!
//! Task Manager and the app list show one process, `vmmemWSL`, holding gigabytes. Most of that
//! is usually Linux's file cache, which Linux keeps because memory it is not using is wasted, and
//! which Windows only gets back when the VM returns it. The Linux side knows the split; this asks
//! it, only when someone opens that row, and only of a distribution that is already running,
//! because starting one to answer a question would be its own surprise.
//!
//! All WSL 2 distributions share one VM and one kernel, so the figures are the VM's, not any one
//! distribution's.

use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use serde::Serialize;

#[derive(Clone, Debug, Default, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct WslMemory {
    pub distros: Vec<String>,
    pub total_bytes: u64,
    /// Memory Linux processes hold: total minus what is free, cached or reclaimable.
    pub used_bytes: u64,
    /// File cache and reclaimable slab: given back when something needs it.
    pub cache_bytes: u64,
    pub free_bytes: u64,
}

const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// Runs `wsl.exe` with a deadline: a WSL service that is starting or stuck must not hold up the
/// window that asked.
fn run(args: &[&str], deadline: Duration) -> Option<Vec<u8>> {
    use std::io::Read;
    use std::os::windows::process::CommandExt;
    let mut child = Command::new("wsl.exe")
        .args(args)
        // Newer WSL writes UTF-16 to a pipe unless told otherwise.
        .env("WSL_UTF8", "1")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .creation_flags(CREATE_NO_WINDOW)
        .spawn()
        .ok()?;
    let started = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if started.elapsed() < deadline => std::thread::sleep(Duration::from_millis(50)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
        }
    }
    let mut out = Vec::new();
    child.stdout.take()?.read_to_end(&mut out).ok()?;
    Some(out)
}

/// `wsl -l` output is UTF-16 on older builds even with `WSL_UTF8` set; both are accepted.
fn decode(bytes: &[u8]) -> String {
    let looks_utf16 = bytes.len() >= 2 && bytes.iter().skip(1).step_by(2).filter(|b| **b == 0).count() * 2 >= bytes.len() / 2;
    if looks_utf16 {
        let units: Vec<u16> = bytes.chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect();
        String::from_utf16_lossy(&units)
    } else {
        String::from_utf8_lossy(bytes).into_owned()
    }
}

pub fn parse_distros(listing: &str) -> Vec<String> {
    listing
        .lines()
        .map(|line| line.trim_matches(|c: char| c.is_whitespace() || c == '\u{feff}' || c == '\0'))
        .filter(|line| !line.is_empty())
        .map(str::to_string)
        .collect()
}

/// `/proc/meminfo` in bytes: used is total minus free, buffers, page cache and reclaimable slab,
/// the same arithmetic `free` does.
pub fn parse_meminfo(text: &str) -> Option<WslMemory> {
    let field = |name: &str| -> Option<u64> {
        let line = text.lines().find(|line| line.starts_with(name) && line[name.len()..].starts_with(':'))?;
        let kb: u64 = line[name.len() + 1..].trim().trim_end_matches("kB").trim().parse().ok()?;
        Some(kb * 1024)
    };
    let total = field("MemTotal")?;
    let free = field("MemFree").unwrap_or(0);
    let cache = field("Cached").unwrap_or(0) + field("Buffers").unwrap_or(0) + field("SReclaimable").unwrap_or(0);
    Some(WslMemory {
        distros: Vec::new(),
        total_bytes: total,
        used_bytes: total.saturating_sub(free).saturating_sub(cache),
        cache_bytes: cache,
        free_bytes: free,
    })
}

/// The VM's memory as Linux sees it, or `None` when WSL is not running or does not answer.
pub fn memory() -> Option<WslMemory> {
    let distros = parse_distros(&decode(&run(&["--list", "--running", "--quiet"], Duration::from_secs(4))?));
    let first = distros.first()?.clone();
    let meminfo = run(&["-d", &first, "--exec", "cat", "/proc/meminfo"], Duration::from_secs(4))?;
    let mut memory = parse_meminfo(&decode(&meminfo))?;
    memory.distros = distros;
    Some(memory)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn meminfo_is_split_the_way_free_splits_it() {
        let text = "MemTotal:       16000000 kB\nMemFree:         2000000 kB\nMemAvailable:   12000000 kB\nBuffers:          100000 kB\nCached:          9000000 kB\nSReclaimable:     400000 kB\n";
        let m = parse_meminfo(text).unwrap();
        assert_eq!(m.total_bytes, 16_000_000 * 1024);
        assert_eq!(m.cache_bytes, 9_500_000 * 1024);
        assert_eq!(m.used_bytes, (16_000_000 - 2_000_000 - 9_500_000) * 1024);
    }

    #[test]
    fn distribution_names_come_out_of_either_encoding() {
        let utf16: Vec<u8> = "Ubuntu\r\ndocker-desktop\r\n".encode_utf16().flat_map(|u| u.to_le_bytes()).collect();
        assert_eq!(parse_distros(&decode(&utf16)), vec!["Ubuntu", "docker-desktop"]);
        assert_eq!(parse_distros(&decode(b"Ubuntu\n\n")), vec!["Ubuntu"]);
    }

    #[test]
    fn a_field_name_must_match_whole() {
        // "Cached" must not read "SwapCached".
        let m = parse_meminfo("MemTotal: 100 kB\nSwapCached: 50 kB\nMemFree: 10 kB\n").unwrap();
        assert_eq!(m.cache_bytes, 0);
    }
}
