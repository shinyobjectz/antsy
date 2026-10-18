//! Machine memory and per-job resident memory, on macOS and Linux.

use std::process::Command;

pub struct Sys {
    pub total_mb: u64,
    pub free_pct: u64,
    /// 1 normal, 2 warn, 4 critical (the macOS scale; Linux is mapped onto it).
    pub pressure: u8,
}

impl Sys {
    pub fn free_mb(&self) -> u64 {
        self.total_mb * self.free_pct / 100
    }
    pub fn line(&self) -> String {
        let p = match self.pressure {
            4 => "critical",
            2 => "warn",
            _ => "normal",
        };
        format!(
            "memory {}% free ({} of {}), pressure {p}",
            self.free_pct,
            crate::util::fmt_mb(self.free_mb()),
            crate::util::fmt_mb(self.total_mb)
        )
    }
}

fn run(cmd: &str, args: &[&str]) -> String {
    Command::new(cmd)
        .args(args)
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).into_owned())
        .unwrap_or_default()
}

#[cfg(target_os = "macos")]
pub fn sys() -> Sys {
    let total_mb = run("sysctl", &["-n", "hw.memsize"]).trim().parse::<u64>().unwrap_or(0) >> 20;
    let free_pct = run("memory_pressure", &["-Q"])
        .split_whitespace()
        .find_map(|w| w.strip_suffix('%').and_then(|n| n.parse().ok()))
        .unwrap_or(100);
    let pressure = run("sysctl", &["-n", "kern.memorystatus_vm_pressure_level"]).trim().parse().unwrap_or(1);
    Sys { total_mb, free_pct, pressure }
}

#[cfg(not(target_os = "macos"))]
pub fn sys() -> Sys {
    let info = std::fs::read_to_string("/proc/meminfo").unwrap_or_default();
    let field = |k: &str| {
        info.lines()
            .find(|l| l.starts_with(k))
            .and_then(|l| l.split_whitespace().nth(1))
            .and_then(|n| n.parse::<u64>().ok())
            .unwrap_or(0)
    };
    let total = field("MemTotal:").max(1);
    let free_pct = field("MemAvailable:") * 100 / total;
    let pressure = if free_pct < 8 { 4 } else if free_pct < 20 { 2 } else { 1 };
    Sys { total_mb: total >> 10, free_pct, pressure }
}

/// Resident memory of every process in a process group, in MB.
pub fn group_rss_mb(pgid: i64) -> u64 {
    let mut kb = 0u64;
    for l in run("ps", &["-A", "-o", "pgid=,rss="]).lines() {
        let mut it = l.split_whitespace();
        if it.next().and_then(|g| g.parse::<i64>().ok()) == Some(pgid) {
            kb += it.next().and_then(|r| r.parse::<u64>().ok()).unwrap_or(0);
        }
    }
    kb >> 10
}

pub fn pid_rss_mb(pid: i64) -> u64 {
    run("ps", &["-o", "rss=", "-p", &pid.to_string()]).trim().parse::<u64>().unwrap_or(0) >> 10
}

pub fn ncpu() -> u64 {
    std::thread::available_parallelism().map(|n| n.get() as u64).unwrap_or(4)
}

/// An external memory killer (memguard, earlyoom, …) is running. Antsy never races one.
pub fn external_killer(names: &[String]) -> Option<String> {
    // Match any argv word's basename: script killers run as `python …/memguard run`.
    let ps = run("ps", &["-A", "-o", "command="]);
    names
        .iter()
        .find(|n| {
            ps.lines().any(|l| {
                l.split_whitespace().take(3).any(|w| w.rsplit('/').next() == Some(n.as_str()))
            })
        })
        .cloned()
}
