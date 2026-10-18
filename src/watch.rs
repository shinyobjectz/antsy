//! `antsy watch`: failure modes for things antsy did not start. The daemon evaluates them, so
//! polling costs no tokens; a session only hears about a watch when it fails or finishes.

use crate::config::{self, get_str, strs};
use crate::mem;
use crate::util::*;
use serde_json::{json, Value};
use std::io::{Read, Seek, SeekFrom};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

const USAGE: &str = "usage:
  antsy watch pid <pid> [--name N] [--mem 8G] [--stall 5m --progress-file F]
  antsy watch log <file> [--name N] [--fail-on RE]... [--until RE] [--stall 2m]
  antsy watch check [--name N] [--every 30s] [--until-ok] [--until RE] [--fail-on RE] -- <command>";

pub fn main(args: &[String]) -> i32 {
    let Some(kind) = args.first().map(String::as_str) else {
        eprintln!("{USAGE}");
        return 2;
    };
    let c = config::load();
    let mut w = json!({
        "kind": kind, "session": session(), "created": now(), "name": "",
        "stall": 0.0, "max": parse_dur(&get_str(&c, "max")), "every": 30.0, "mem_mb": 0,
        "fail_on": [], "until": [], "until_ok": false, "progress_file": "",
        "last_progress": now(), "last_run": 0.0, "offset": 0, "target": "",
    });
    let mut it = args[1..].iter();
    while let Some(a) = it.next() {
        let mut val = || it.next().cloned().unwrap_or_default();
        match a.as_str() {
            "--" => {
                let rest: Vec<String> = it.by_ref().cloned().collect();
                w["target"] = json!(rest.join(" "));
            }
            "--name" | "-n" => w["name"] = json!(val()),
            "--stall" => w["stall"] = json!(parse_dur(&val())),
            "--max" => w["max"] = json!(parse_dur(&val())),
            "--every" => w["every"] = json!(parse_dur(&val()).max(2.0)),
            "--mem" => w["mem_mb"] = json!(parse_mem(&val())),
            "--fail-on" => w["fail_on"].as_array_mut().unwrap().push(json!(val())),
            "--until" => w["until"].as_array_mut().unwrap().push(json!(val())),
            "--until-ok" => w["until_ok"] = json!(true),
            "--progress-file" => w["progress_file"] = json!(val()),
            _ if a.starts_with('-') => {
                eprintln!("unknown flag {a}\n{USAGE}");
                return 2;
            }
            _ => w["target"] = json!(a),
        }
    }
    // The daemon runs elsewhere: pin relative paths and checks to this directory.
    let cwd = std::env::current_dir().unwrap_or_default();
    w["cwd"] = json!(cwd.display().to_string());
    if kind == "log" {
        let abs = cwd.join(s(&w, "target"));
        w["target"] = json!(abs.display().to_string());
    }
    let target = s(&w, "target").to_string();
    match kind {
        "pid" if target.parse::<i64>().map(alive).unwrap_or(false) => {}
        "pid" => {
            eprintln!("antsy watch: pid {target} is not running");
            return 2;
        }
        "log" => {
            let len = std::fs::metadata(&target).map(|m| m.len()).unwrap_or(0);
            w["offset"] = json!(len);
            if w["fail_on"].as_array().unwrap().is_empty() {
                w["fail_on"] = json!(strs(&c, "fail_on"));
            }
            if f(&w, "stall") == 0.0 {
                w["stall"] = json!(parse_dur(&get_str(&c, "stall")));
            }
        }
        "check" if !target.is_empty() => {}
        _ => {
            eprintln!("{USAGE}");
            return 2;
        }
    }
    if s(&w, "name").is_empty() {
        w["name"] = json!(format!("{kind} {}", target.chars().take(40).collect::<String>()));
    }
    let id = new_id(s(&w, "name"));
    w["id"] = json!(id);
    write_json(&dir("watches").join(format!("{id}.json")), &w);
    crate::daemon::ensure();
    emit("start", &session(), &id, "watch", s(&w, "name"));
    println!("{id}");
    0
}

/// One evaluation. Returns the final event (kind, mode, message) when the watch is over.
pub fn tick(w: &mut Value) -> Option<(&'static str, String, String)> {
    let name = s(w, "name").to_string();
    let target = s(w, "target").to_string();
    let age = now() - f(w, "created");
    if f(w, "max") > 0.0 && age > f(w, "max") {
        return Some(("fail", "max".into(), format!("{name}: still unresolved at the {} ceiling", fmt_ago(f(w, "max")))));
    }
    let pf = s(w, "progress_file").to_string();
    if !pf.is_empty() {
        if let Ok(m) = std::fs::metadata(&pf).and_then(|m| m.modified()) {
            let t = m.duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs_f64()).unwrap_or(0.0);
            if t > f(w, "last_progress") {
                w["last_progress"] = json!(t);
            }
        }
    }
    match s(w, "kind") {
        "pid" => {
            let pid: i64 = target.parse().unwrap_or(0);
            if !alive(pid) {
                return Some(("done", "exited".into(), format!("{name}: pid {pid} exited (exit status unknown to a watcher; check its output)")));
            }
            let mb = f(w, "mem_mb") as u64;
            if mb > 0 {
                let rss = mem::pid_rss_mb(pid);
                w["rss_mb"] = json!(rss);
                if rss > mb {
                    return Some(("fail", "mem".into(), format!("{name}: pid {pid} at {} > {} (not killed; stop it or raise --mem)", fmt_mb(rss), fmt_mb(mb))));
                }
            }
        }
        "log" => {
            let mut file = std::fs::File::open(&target).ok()?;
            let len = file.metadata().map(|m| m.len()).unwrap_or(0);
            let off = i(w, "offset") as u64;
            if len < off {
                w["offset"] = json!(0);
            } else if len > off {
                let mut buf = vec![];
                let _ = file.seek(SeekFrom::Start(off));
                let _ = file.take(len - off).read_to_end(&mut buf);
                // Only whole lines; a partial last line is read next time.
                let upto = buf.iter().rposition(|&b| b == b'\n').map(|p| p + 1).unwrap_or(0);
                w["offset"] = json!(off + upto as u64);
                if upto > 0 {
                    w["last_progress"] = json!(now());
                }
                let text = String::from_utf8_lossy(&buf[..upto]).into_owned();
                let fails = config::regexes(&strs(w, "fail_on"));
                let untils = config::regexes(&strs(w, "until"));
                for line in text.lines() {
                    if fails.iter().any(|r| r.is_match(line)) {
                        return Some(("fail", "fail_on".into(), format!("{name}: {line}")));
                    }
                    if untils.iter().any(|r| r.is_match(line)) {
                        return Some(("done", "until".into(), format!("{name}: {line}")));
                    }
                }
            }
        }
        "check" => {
            if now() - f(w, "last_run") < f(w, "every") {
                return None;
            }
            w["last_run"] = json!(now());
            let (code, out) = run_check(&target, s(w, "cwd"), Duration::from_secs_f64(f(w, "every").clamp(5.0, 60.0)));
            let last = out.lines().last().unwrap_or("").chars().take(300).collect::<String>();
            if config::regexes(&strs(w, "fail_on")).iter().any(|r| r.is_match(&out)) {
                return Some(("fail", "fail_on".into(), format!("{name}: {last}")));
            }
            if config::regexes(&strs(w, "until")).iter().any(|r| r.is_match(&out)) {
                return Some(("done", "until".into(), format!("{name}: {last}")));
            }
            let until_ok = w["until_ok"].as_bool().unwrap_or(false);
            match (code, until_ok) {
                (0, true) => return Some(("done", "ok".into(), format!("{name}: check passed {last}"))),
                (0, false) => w["last_progress"] = json!(now()),
                (c, false) => return Some(("fail", "check".into(), format!("{name}: check exited {c}: {last}"))),
                (_, true) => {}
            }
        }
        _ => return Some(("fail", "bad".into(), format!("{name}: unknown watch kind"))),
    }
    let quiet = now() - f(w, "last_progress");
    if f(w, "stall") > 0.0 && quiet > f(w, "stall") {
        return Some(("fail", "stall".into(), format!("{name}: no progress for {}", fmt_ago(quiet))));
    }
    None
}

fn run_check(cmd: &str, cwd: &str, limit: Duration) -> (i32, String) {
    use std::os::unix::process::CommandExt;
    let Ok(mut ch) = Command::new("sh")
        .args(["-c", &format!("{cmd} 2>&1")])
        .current_dir(if cwd.is_empty() { "/" } else { cwd })
        .process_group(0)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .spawn()
    else {
        return (127, "cannot start check".into());
    };
    let pgid = ch.id() as i64;
    let mut out = ch.stdout.take().unwrap();
    let reader = std::thread::spawn(move || {
        let mut s = String::new();
        let _ = out.read_to_string(&mut s);
        s
    });
    let start = Instant::now();
    let code = loop {
        if let Ok(Some(st)) = ch.try_wait() {
            break st.code().unwrap_or(1);
        }
        if start.elapsed() > limit {
            kill_group(pgid, libc::SIGKILL);
            let _ = ch.wait();
            break 124;
        }
        std::thread::sleep(Duration::from_millis(100));
    };
    kill_group(pgid, libc::SIGKILL);
    (code, reader.join().unwrap_or_default())
}
