//! `antsy daemon`: one per user. It checks failure modes so no session spends tokens polling:
//! vanished supervisors, watches, machine memory, and OTLP traces/logs posted to 127.0.0.1:<otlp_port>.

use crate::config::{self, get_f, get_str, strs};
use crate::util::*;
use crate::{mem, otel, watch};
use serde_json::{json, Value};
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

fn pid_path() -> std::path::PathBuf {
    home().join("daemon.pid")
}

/// (pid, version) from the pid file.
fn recorded() -> Option<(i64, String)> {
    let t = std::fs::read_to_string(pid_path()).ok()?;
    let mut l = t.lines();
    Some((l.next()?.trim().parse().ok()?, l.next().unwrap_or("").trim().to_string()))
}

pub fn running() -> bool {
    recorded().map(|(p, _)| alive(p)).unwrap_or(false)
}

/// Start the daemon in the background if it is not up. Never waits on it.
pub fn ensure() {
    if std::env::var("ANTSY_NO_DAEMON").is_ok() {
        return;
    }
    match recorded() {
        Some((pid, v)) if alive(pid) && v == env!("CARGO_PKG_VERSION") => return,
        // An older daemon steps aside when the pid file stops naming it.
        Some((pid, _)) if alive(pid) => {
            let _ = std::fs::remove_file(pid_path());
        }
        _ => {}
    }
    if let Ok(exe) = std::env::current_exe() {
        use std::os::unix::process::CommandExt;
        let log = std::fs::OpenOptions::new().create(true).append(true).open(home().join("daemon.log"));
        let mut c = Command::new(exe);
        c.arg("daemon").process_group(0).stdin(Stdio::null()).stdout(Stdio::null());
        match log {
            Ok(f) => c.stderr(f),
            Err(_) => c.stderr(Stdio::null()),
        };
        let _ = c.spawn();
    }
}

pub fn main(_args: &[String]) -> i32 {
    let _ = std::fs::create_dir_all(home());
    if running() {
        eprintln!("antsy daemon already running");
        return 0;
    }
    let me = std::process::id().to_string();
    std::fs::write(pid_path(), format!("{me}\n{}\n", env!("CARGO_PKG_VERSION"))).ok();
    let c = config::load();
    let port = get_str(&c, "otlp_port");
    let last_activity = Arc::new(AtomicU64::new(now() as u64));
    let act = last_activity.clone();
    std::thread::spawn(move || {
        // A daemon we just replaced may hold the port for one more tick.
        let mut tries = 0;
        let l = loop {
            match TcpListener::bind(format!("127.0.0.1:{port}")) {
                Ok(l) => break l,
                Err(e) if tries >= 10 => {
                    eprintln!("antsy daemon: OTLP port {port} unavailable ({e}); traces from programs won't be read");
                    return;
                }
                Err(_) => {
                    tries += 1;
                    std::thread::sleep(Duration::from_secs(1));
                }
            }
        };
        for stream in l.incoming().flatten() {
            let act = act.clone();
            std::thread::spawn(move || {
                act.store(now() as u64, Ordering::Relaxed);
                let _ = serve(stream);
            });
        }
    });
    let idle_exit = parse_dur(&get_str(&c, "idle_exit"));
    let mut mem_state = 1u8;
    let mut mem_warned = 0.0;
    let mut tick = 0u64;
    loop {
        let busy = check_jobs() | check_watches();
        if busy {
            last_activity.store(now() as u64, Ordering::Relaxed);
        }
        if tick % 3 == 0 {
            check_memory(&c, &mut mem_state, &mut mem_warned);
        }
        if tick % 150 == 0 {
            prune();
        }
        tick += 1;
        if idle_exit > 0.0 && now() - last_activity.load(Ordering::Relaxed) as f64 > idle_exit {
            break;
        }
        // Someone replaced us (a newer binary): step aside.
        if recorded().map(|(p, _)| p.to_string() != me).unwrap_or(true) {
            return 0;
        }
        std::thread::sleep(Duration::from_secs(2));
    }
    let _ = std::fs::remove_file(pid_path());
    0
}

/// A job whose supervisor died (SIGKILL, OOM killer, closed terminal) is failed, and its process group killed.
fn check_jobs() -> bool {
    let mut busy = false;
    for (p, mut j) in list("jobs") {
        if s(&j, "status") != "running" {
            continue;
        }
        busy = true;
        if !alive(i(&j, "pid")) {
            kill_group(i(&j, "pgid"), libc::SIGKILL);
            j["status"] = json!("failed");
            j["mode"] = json!("vanished");
            j["ended"] = json!(now());
            write_json(&p, &j);
            emit(
                "fail",
                s(&j, "session"),
                s(&j, "id"),
                "vanished",
                &format!("{}: supervisor died (killed from outside?); its processes were stopped. log {}", s(&j, "name"), s(&j, "log")),
            );
        }
    }
    busy
}

fn check_watches() -> bool {
    let mut busy = false;
    for (p, mut w) in list("watches") {
        busy = true;
        match watch::tick(&mut w) {
            Some((kind, mode, msg)) => {
                let _ = std::fs::remove_file(&p);
                emit(kind, s(&w, "session"), s(&w, "id"), &mode, &msg);
            }
            None => write_json(&p, &w),
        }
    }
    busy
}

fn check_memory(c: &Value, state: &mut u8, warned: &mut f64) {
    let sys = mem::sys();
    let floor = get_f(c, "free_floor_pct") as u64;
    let level = if sys.pressure >= 4 { 4 } else if sys.free_pct < floor || sys.pressure >= 2 { 2 } else { 1 };
    if level > 1 && (level > *state || now() - *warned > 300.0) {
        *warned = now();
        emit("warn", "*", "", "memory", &format!("{} — start nothing heavy; `antsy board` shows who holds what", sys.line()));
    }
    *state = level;
    if level < 4 {
        return;
    }
    let killer = get_str(c, "mem_killer");
    let external = mem::external_killer(&strs(c, "external_killers"));
    if killer == "off" || (killer != "antsy" && external.is_some()) {
        return;
    }
    // At the edge, stop the largest antsy job over 1 GB. Only antsy's own jobs, never anything else.
    let biggest = crate::board::running_jobs().into_iter().filter(|j| i(j, "rss_mb") > 1024).max_by_key(|j| i(j, "rss_mb"));
    if let Some(j) = biggest {
        let stop = dir("jobs").join(format!("{}.stop", s(&j, "id")));
        let _ = std::fs::write(stop, format!("memory edge ({}), largest antsy job at {}", sys.line(), fmt_mb(i(&j, "rss_mb") as u64)));
    }
}

/// Finished jobs and their logs go after a week; the trace files after two.
fn prune() {
    let week = 7.0 * 86400.0;
    for (p, j) in list("jobs") {
        if s(&j, "status") != "running" && now() - f(&j, "ended").max(f(&j, "started")) > week {
            let _ = std::fs::remove_file(s(&j, "log"));
            let _ = std::fs::remove_file(p);
        }
    }
    for sub in ["traces", "waiters"] {
        if let Ok(rd) = std::fs::read_dir(dir(sub)) {
            for e in rd.flatten() {
                let old = e.metadata().and_then(|m| m.modified()).ok().and_then(|m| m.elapsed().ok()).map(|d| d.as_secs_f64() > 2.0 * week);
                if old == Some(true) {
                    let _ = std::fs::remove_file(e.path());
                }
            }
        }
    }
}

fn serve(mut stream: TcpStream) -> std::io::Result<()> {
    stream.set_read_timeout(Some(Duration::from_secs(10)))?;
    let mut r = BufReader::new(stream.try_clone()?);
    let mut first = String::new();
    r.read_line(&mut first)?;
    let mut len = 0usize;
    let mut ctype = String::new();
    let mut encoding = String::new();
    loop {
        let mut h = String::new();
        if r.read_line(&mut h)? == 0 || h == "\r\n" || h == "\n" {
            break;
        }
        let (k, v) = h.split_once(':').unwrap_or((&h, ""));
        match k.trim().to_ascii_lowercase().as_str() {
            "content-length" => len = v.trim().parse().unwrap_or(0),
            "content-type" => ctype = v.trim().to_string(),
            "content-encoding" => encoding = v.trim().to_string(),
            _ => {}
        }
    }
    let mut body = vec![0u8; len.min(64 << 20)];
    r.read_exact(&mut body)?;
    let path = first.split_whitespace().nth(1).unwrap_or("/").to_string();
    let mut reply = |code: &str, b: &str| {
        let _ = write!(stream, "HTTP/1.1 {code}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{b}", b.len());
    };
    if path == "/health" {
        reply("200 OK", "{\"ok\":true}");
        return Ok(());
    }
    let kind = match path.as_str() {
        "/v1/traces" => "traces",
        "/v1/logs" => "logs",
        "/v1/metrics" => "metrics",
        _ => {
            reply("404 Not Found", "{}");
            return Ok(());
        }
    };
    // Protobuf or compressed batches are accepted (so exporters don't retry) but can't be read.
    if !ctype.contains("json") || !(encoding.is_empty() || encoding == "identity") {
        otel::store(kind, &json!({"unreadable": format!("{ctype} {encoding}"), "bytes": len}));
        reply("200 OK", "{}");
        return Ok(());
    }
    let v: Value = serde_json::from_slice(&body).unwrap_or(Value::Null);
    otel::store(kind, &v);
    let failures = match kind {
        "traces" => otel::span_failures(&v),
        "logs" => otel::log_failures(&v),
        _ => vec![],
    };
    for (sess, job, msg) in failures.into_iter().take(20) {
        emit("fail", &sess, &job, "otel", &msg);
    }
    reply("200 OK", "{}");
    Ok(())
}
