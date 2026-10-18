//! Paths, state files, events and small helpers shared by every command.

use serde_json::{json, Value};
use std::collections::hash_map::RandomState;
use std::fs::{self, OpenOptions};
use std::hash::{BuildHasher, Hasher};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

pub fn home() -> PathBuf {
    if let Ok(h) = std::env::var("ANTSY_HOME") {
        return PathBuf::from(h);
    }
    let base = std::env::var("HOME").unwrap_or_else(|_| "/tmp".into());
    PathBuf::from(base).join(".antsy")
}

/// A folder under the antsy home, created on first use.
pub fn dir(sub: &str) -> PathBuf {
    let d = home().join(sub);
    let _ = fs::create_dir_all(&d);
    d
}

pub fn now() -> f64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs_f64()).unwrap_or(0.0)
}

pub fn now_ns() -> u128 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0)
}

/// The Claude Code session this process belongs to: the env var Claude Code sets for Bash (it wins over
/// an inherited ANTSY_SESSION, so a nested session is its own), else the
/// session file of the nearest Claude Code ancestor (monitors and other children), else "none".
pub fn session() -> String {
    static S: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    S.get_or_init(|| {
        for k in ["CLAUDE_CODE_SESSION_ID", "ANTSY_SESSION"] {
            if let Ok(v) = std::env::var(k) {
                if !v.is_empty() {
                    return v;
                }
            }
        }
        let sessions = PathBuf::from(std::env::var("HOME").unwrap_or_default()).join(".claude/sessions");
        let from_pid = |pid: i64| read_json(&sessions.join(format!("{pid}.json"))).map(|v| s(&v, "sessionId").to_string());
        if let Some(id) = std::env::var("CLAUDE_PID").ok().and_then(|p| p.parse().ok()).and_then(from_pid) {
            return id;
        }
        let mut pid = std::process::id() as i64;
        for _ in 0..8 {
            pid = ppid_of(pid);
            if pid <= 1 {
                break;
            }
            if let Some(id) = from_pid(pid).filter(|x| !x.is_empty()) {
                return id;
            }
        }
        "none".into()
    })
    .clone()
}

pub fn short(s: &str) -> &str {
    &s[..s.len().min(8)]
}

pub fn rand64() -> u64 {
    let mut h = RandomState::new().build_hasher();
    h.write_u128(now_ns());
    h.write_u32(std::process::id());
    h.finish()
}

pub fn hex(n: u64) -> String {
    format!("{n:016x}")
}

pub fn new_id(name: &str) -> String {
    let slug: String = name
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c.to_ascii_lowercase() } else { '-' })
        .collect::<String>()
        .split('-')
        .filter(|s| !s.is_empty())
        .take(4)
        .collect::<Vec<_>>()
        .join("-");
    let slug = if slug.is_empty() { "job".to_string() } else { slug };
    format!("{}-{}", &slug[..slug.len().min(24)], &hex(rand64())[..5])
}

pub fn read_json(p: &Path) -> Option<Value> {
    serde_json::from_str(&fs::read_to_string(p).ok()?).ok()
}

pub fn write_json(p: &Path, v: &Value) {
    let tmp = p.with_extension(format!("tmp{}", std::process::id()));
    if fs::write(&tmp, v.to_string()).is_ok() {
        let _ = fs::rename(&tmp, p);
    }
}

pub fn append_line(p: &Path, v: &Value) {
    if let Ok(mut f) = OpenOptions::new().create(true).append(true).open(p) {
        let mut s = v.to_string();
        s.push('\n');
        let _ = f.write_all(s.as_bytes());
    }
}

pub fn events_path() -> PathBuf {
    home().join("events.jsonl")
}

/// Record something a session should hear about. `session` "*" reaches every session.
pub fn emit(kind: &str, session: &str, job: &str, mode: &str, msg: &str) {
    let _ = fs::create_dir_all(home());
    let p = events_path();
    if fs::metadata(&p).map(|m| m.len() > 8 << 20).unwrap_or(false) {
        let _ = fs::rename(&p, home().join("events.1.jsonl"));
    }
    append_line(
        &p,
        &json!({"ts": now(), "kind": kind, "session": session, "job": job, "mode": mode, "msg": msg}),
    );
}

/// Every JSON file in a state folder, e.g. all jobs.
pub fn list(sub: &str) -> Vec<(PathBuf, Value)> {
    let mut out = vec![];
    if let Ok(rd) = fs::read_dir(dir(sub)) {
        for e in rd.flatten() {
            let p = e.path();
            if p.extension().map(|x| x == "json").unwrap_or(false) {
                if let Some(v) = read_json(&p) {
                    out.push((p, v));
                }
            }
        }
    }
    out
}

pub fn alive(pid: i64) -> bool {
    pid > 0
        && (unsafe { libc::kill(pid as i32, 0) } == 0
            || std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM))
}

pub fn kill_group(pgid: i64, sig: i32) {
    if pgid > 1 {
        unsafe {
            libc::kill(-(pgid as i32), sig);
        }
    }
}

pub fn ppid_of(pid: i64) -> i64 {
    let out = std::process::Command::new("ps").args(["-o", "ppid=", "-p", &pid.to_string()]).output();
    out.ok().and_then(|o| String::from_utf8_lossy(&o.stdout).trim().parse().ok()).unwrap_or(0)
}

/// "90", "90s", "5m", "2h", "1d" -> seconds. "0", "off" or "none" -> 0 (disabled).
pub fn parse_dur(s: &str) -> f64 {
    let s = s.trim();
    if matches!(s, "" | "off" | "none") {
        return 0.0;
    }
    let (n, unit) = s.split_at(s.find(|c: char| c.is_ascii_alphabetic()).unwrap_or(s.len()));
    let n: f64 = n.parse().unwrap_or(0.0);
    n * match unit {
        "ms" => 0.001,
        "m" | "min" => 60.0,
        "h" => 3600.0,
        "d" => 86400.0,
        _ => 1.0,
    }
}

/// "512M", "4G", "4GB", "1.5g" -> megabytes. A bare number is megabytes.
pub fn parse_mem(s: &str) -> u64 {
    let s = s.trim().to_ascii_uppercase();
    let s = s.trim_end_matches('B');
    let (n, unit) = s.split_at(s.find(|c: char| c.is_ascii_alphabetic()).unwrap_or(s.len()));
    let n: f64 = n.parse().unwrap_or(0.0);
    (n * match unit {
        "K" => 1.0 / 1024.0,
        "G" => 1024.0,
        "T" => 1024.0 * 1024.0,
        _ => 1.0,
    }) as u64
}

pub fn fmt_mb(mb: u64) -> String {
    if mb >= 1024 {
        format!("{:.1} GB", mb as f64 / 1024.0)
    } else {
        format!("{mb} MB")
    }
}

/// Elapsed time, for describing what already happened. Never used to predict.
pub fn fmt_ago(secs: f64) -> String {
    let s = secs.max(0.0) as u64;
    match s {
        0..=99 => format!("{s}s"),
        100..=5999 => format!("{}m{:02}s", s / 60, s % 60),
        _ => format!("{}h{:02}m", s / 3600, (s % 3600) / 60),
    }
}

/// Days since 1970-01-01 -> (year, month, day), proleptic Gregorian.
pub fn civil(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (yoe + era * 400 + i64::from(m <= 2), m, d)
}

pub fn today() -> String {
    let (y, m, d) = civil((now() / 86400.0) as i64);
    format!("{y:04}-{m:02}-{d:02}")
}

pub fn tail_lines(p: &Path, n: usize) -> Vec<String> {
    let s = fs::read(p).map(|b| String::from_utf8_lossy(&b).into_owned()).unwrap_or_default();
    let lines: Vec<&str> = s.lines().collect();
    lines[lines.len().saturating_sub(n)..].iter().map(|l| l.to_string()).collect()
}

pub fn s<'a>(v: &'a Value, k: &str) -> &'a str {
    v.get(k).and_then(Value::as_str).unwrap_or("")
}

pub fn f(v: &Value, k: &str) -> f64 {
    v.get(k).and_then(Value::as_f64).unwrap_or(0.0)
}

pub fn i(v: &Value, k: &str) -> i64 {
    v.get(k).and_then(Value::as_i64).unwrap_or(0)
}
