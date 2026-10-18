//! Getting failures to the session that owns them: `antsy wait`, `antsy events --follow` (for the
//! Monitor tool and the plugin monitor), and the cursor the hooks deliver from.

use crate::board;
use crate::util::*;
use serde_json::{json, Value};
use std::io::{Read, Seek, SeekFrom};
use std::time::Duration;

/// Event kinds worth a session's attention. "start" is bookkeeping only.
fn notable(e: &Value) -> bool {
    matches!(s(e, "kind"), "fail" | "ok" | "done" | "warn")
}

pub fn for_session(e: &Value, sess: &str) -> bool {
    let es = s(e, "session");
    es == "*" || es == sess || sess == "*"
}

pub fn format(e: &Value) -> String {
    let tag = match s(e, "kind") {
        "fail" => "FAILED",
        "warn" => "WARNING",
        "ok" => "ok",
        "done" => "done",
        k => k,
    };
    let job = s(e, "job");
    let hint = if s(e, "kind") == "fail" && !job.is_empty() && dir("jobs").join(format!("{job}.json")).exists() {
        format!(" → `antsy log {job}`")
    } else {
        String::new()
    };
    format!("antsy {tag}: {}{hint}", s(e, "msg"))
}

/// Read events from a byte offset; returns them and the new offset. A rotated file restarts at 0.
pub fn read_from(off: u64) -> (Vec<Value>, u64) {
    let Ok(mut file) = std::fs::File::open(events_path()) else { return (vec![], 0) };
    let len = file.metadata().map(|m| m.len()).unwrap_or(0);
    let off = if off > len { 0 } else { off };
    let mut buf = vec![];
    let _ = file.seek(SeekFrom::Start(off));
    let _ = file.take(len - off).read_to_end(&mut buf);
    let upto = buf.iter().rposition(|&b| b == b'\n').map(|p| p + 1).unwrap_or(0);
    let evs = String::from_utf8_lossy(&buf[..upto]).lines().filter_map(|l| serde_json::from_str(l).ok()).collect();
    (evs, off + upto as u64)
}

pub fn end_offset() -> u64 {
    std::fs::metadata(events_path()).map(|m| m.len()).unwrap_or(0)
}

fn cursor_path(sess: &str) -> std::path::PathBuf {
    dir("cursors").join(format!("{}.json", sess.replace('/', "_")))
}

/// Notable events this session hasn't been told about yet; advances its cursor.
pub fn undelivered(sess: &str, max: usize) -> Vec<String> {
    let cp = cursor_path(sess);
    let Some(off) = read_json(&cp).map(|v| i(&v, "offset") as u64) else {
        write_json(&cp, &json!({"offset": end_offset()}));
        return vec![];
    };
    if off == end_offset() {
        return vec![];
    }
    let (evs, new) = read_from(off);
    write_json(&cp, &json!({"offset": new}));
    let mut lines: Vec<String> = evs.iter().filter(|e| notable(e) && for_session(e, sess)).map(format).collect();
    if lines.len() > max {
        let extra = lines.len() - max;
        lines.truncate(max);
        lines.push(format!("…and {extra} more: `antsy events`"));
    }
    lines
}

pub fn start_cursor(sess: &str) {
    let cp = cursor_path(sess);
    if read_json(&cp).is_none() {
        write_json(&cp, &json!({"offset": end_offset()}));
    }
}

/// While this process lives, the session has someone listening for its failures.
fn register_waiter(sess: &str) -> std::path::PathBuf {
    let p = dir("waiters").join(format!("{}-{}", sess.replace('/', "_"), std::process::id()));
    let _ = std::fs::write(&p, "");
    p
}

pub fn has_waiter(sess: &str) -> bool {
    let prefix = format!("{}-", sess.replace('/', "_"));
    std::fs::read_dir(dir("waiters"))
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| e.file_name().into_string().ok())
        .filter(|n| n.starts_with(&prefix))
        .any(|n| {
            let alive_ = n[prefix.len()..].parse().map(alive).unwrap_or(false);
            if !alive_ {
                let _ = std::fs::remove_file(dir("waiters").join(&n));
            }
            alive_
        })
}

pub fn mine_running(sess: &str) -> (Vec<Value>, Vec<Value>) {
    // Inside an antsy job, that job is not something to wait on: it would wait on itself.
    let me = std::env::var("ANTSY_JOB").unwrap_or_default();
    let jobs = board::running_jobs().into_iter().filter(|j| s(j, "session") == sess && s(j, "id") != me).collect();
    let ws = board::watches().into_iter().filter(|w| s(w, "session") == sess).collect();
    (jobs, ws)
}

/// `antsy wait [ID…]`: with IDs, block until all of them end (exit 1 as soon as one fails); without,
/// block until this session's next result or failure. Run it in the background to be woken. It
/// never waits on nothing: with nothing running it returns at once.
pub fn wait(args: &[String]) -> i32 {
    let sess = session();
    let ids: Vec<&String> = args.iter().filter(|a| !a.starts_with('-')).collect();
    let pending = || -> usize {
        if ids.is_empty() {
            let (jobs, ws) = mine_running(&sess);
            jobs.len() + ws.len()
        } else {
            let all: Vec<Value> = board::running_jobs().into_iter().chain(board::watches()).collect();
            ids.iter().filter(|id| all.iter().any(|x| s(x, "id") == id.as_str())).count()
        }
    };
    let matches = |e: &Value| {
        notable(e)
            && if ids.is_empty() {
                s(e, "kind") != "warn" && s(e, "session") == sess
            } else {
                ids.iter().any(|id| id.as_str() == s(e, "job"))
            }
    };
    if pending() == 0 {
        println!("antsy: nothing running to wait for");
        return 0;
    }
    let waiter = register_waiter(&sess);
    let mut off = end_offset();
    let mut code = 0;
    loop {
        let left = pending();
        let (evs, new) = read_from(off);
        off = new;
        let mut hit = false;
        for e in evs.iter().filter(|e| matches(e)) {
            println!("{}", format(e));
            hit = true;
            if s(e, "kind") == "fail" {
                code = 1;
            }
        }
        if code == 1 || (hit && ids.is_empty()) || left == 0 {
            break;
        }
        std::thread::sleep(Duration::from_millis(500));
    }
    let _ = std::fs::remove_file(waiter);
    code
}

/// `antsy events [--follow] [--all] [--session S] [-n N]`
pub fn events(args: &[String]) -> i32 {
    let mut sess = session();
    let mut follow = false;
    let mut n = 20usize;
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--follow" | "-f" => follow = true,
            "--all" => sess = "*".into(),
            "--session" => sess = it.next().cloned().unwrap_or(sess),
            "-n" => n = it.next().and_then(|x| x.parse().ok()).unwrap_or(n),
            _ => {}
        }
    }
    if follow {
        crate::daemon::ensure();
        let _waiter = register_waiter(&sess);
        let mut off = end_offset();
        loop {
            let (evs, new) = read_from(off);
            off = new;
            for e in evs.iter().filter(|e| notable(e) && for_session(e, &sess)) {
                println!("{}", format(e));
            }
            use std::io::Write;
            if std::io::stdout().flush().is_err() {
                return 0;
            }
            std::thread::sleep(Duration::from_millis(500));
        }
    }
    let (evs, _) = read_from(0);
    let mine: Vec<&Value> = evs.iter().filter(|e| notable(e) && for_session(e, &sess)).collect();
    for e in &mine[mine.len().saturating_sub(n)..] {
        println!("{} {}", fmt_ago(now() - f(e, "ts")) + " ago", format(e));
    }
    0
}

/// `antsy stop <ID>… | --mine`
pub fn stop(args: &[String]) -> i32 {
    let sess = session();
    let mine = args.iter().any(|a| a == "--mine");
    let mut n = 0;
    for j in board::running_jobs() {
        if (mine && s(&j, "session") == sess) || args.iter().any(|a| a == s(&j, "id")) {
            let _ = std::fs::write(dir("jobs").join(format!("{}.stop", s(&j, "id"))), "antsy stop");
            n += 1;
        }
    }
    for (p, w) in list("watches") {
        if (mine && s(&w, "session") == sess) || args.iter().any(|a| a == s(&w, "id")) {
            let _ = std::fs::remove_file(p);
            emit("done", s(&w, "session"), s(&w, "id"), "stopped", &format!("{}: watch removed", s(&w, "name")));
            n += 1;
        }
    }
    println!("antsy: stopping {n}");
    if n == 0 {
        1
    } else {
        0
    }
}

/// `antsy log <ID> [-n N]`
pub fn log(args: &[String]) -> i32 {
    let Some(id) = args.first() else {
        eprintln!("usage: antsy log <job id> [-n lines]");
        return 2;
    };
    let n = args.iter().position(|a| a == "-n").and_then(|p| args.get(p + 1)).and_then(|x| x.parse().ok()).unwrap_or(40);
    let Some(j) = read_json(&dir("jobs").join(format!("{id}.json"))) else {
        eprintln!("antsy: no job {id}");
        return 1;
    };
    println!(
        "{} [{}{}] exit {} peak {} — {}",
        s(&j, "name"),
        s(&j, "status"),
        if s(&j, "mode").is_empty() { String::new() } else { format!(": {}", s(&j, "mode")) },
        i(&j, "exit"),
        fmt_mb(i(&j, "peak_mb") as u64),
        s(&j, "why")
    );
    for l in tail_lines(std::path::Path::new(s(&j, "log")), n) {
        println!("{l}");
    }
    0
}
