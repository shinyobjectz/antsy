//! `antsy run`: run a command under a failure-mode contract.
//!
//! The job fails fast on the first of: a fail_on line, no progress for `stall`, its process group over
//! `mem`, the `max` ceiling, a stop request, or a signal to the supervisor. Every outcome has its own
//! exit code, an event for the owning session, a log, and an OTLP span.

use crate::config::{self, get_f, get_str, strs};
use crate::util::*;
use crate::{daemon, mem, otel};
use serde_json::{json, Value};
use std::io::{BufRead, BufReader, Write};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicI32, Ordering};
use std::sync::mpsc;
use std::time::{Duration, Instant};

pub const EXIT_STALL: i32 = 121;
pub const EXIT_MEM: i32 = 122;
pub const EXIT_FAIL_ON: i32 = 123;
pub const EXIT_MAX: i32 = 124;
pub const EXIT_REFUSED: i32 = 125;
pub const EXIT_STOPPED: i32 = 126;

static SIGNAL: AtomicI32 = AtomicI32::new(0);

extern "C" fn on_signal(sig: i32) {
    SIGNAL.store(sig, Ordering::SeqCst);
}

pub struct Opts {
    pub name: String,
    pub stall: f64,
    pub max: f64,
    pub mem_mb: u64,
    pub need_mb: u64,
    pub fail_on: Vec<String>,
    pub defaults: bool,
    pub grace: f64,
    pub quiet: bool,
    pub progress_file: Option<String>,
    pub cost_usd: f64,
    /// What this run is for: the question it answers or the goal it serves (attribution).
    pub purpose: String,
    pub cmd: Vec<String>,
}

pub fn parse(args: &[String]) -> Result<Opts, String> {
    let c = config::load();
    let mut o = Opts {
        name: String::new(),
        stall: parse_dur(&get_str(&c, "stall")),
        max: parse_dur(&get_str(&c, "max")),
        mem_mb: 0,
        need_mb: 0,
        fail_on: vec![],
        defaults: true,
        grace: parse_dur(&get_str(&c, "grace")),
        quiet: false,
        progress_file: None,
        cost_usd: 0.0,
        purpose: std::env::var("ANTSY_FOR").unwrap_or_default(),
        cmd: vec![],
    };
    let mut it = args.iter();
    while let Some(a) = it.next() {
        let mut val = || it.next().cloned().ok_or(format!("{a} needs a value"));
        match a.as_str() {
            "--" => {
                o.cmd = it.by_ref().cloned().collect();
                break;
            }
            "--name" | "-n" => o.name = val()?,
            "--stall" => o.stall = parse_dur(&val()?),
            "--max" => o.max = parse_dur(&val()?),
            "--mem" => o.mem_mb = parse_mem(&val()?),
            "--need-mem" => o.need_mb = parse_mem(&val()?),
            "--fail-on" => o.fail_on.push(val()?),
            "--no-default-fail-on" => o.defaults = false,
            "--grace" => o.grace = parse_dur(&val()?),
            "--progress-file" => o.progress_file = Some(val()?),
            "--cost-usd" => o.cost_usd = val()?.parse().map_err(|_| "--cost-usd takes dollars")?,
            "--for" | "--purpose" => o.purpose = val()?,
            "--quiet" | "-q" => o.quiet = true,
            _ if a.starts_with('-') => return Err(format!("unknown flag {a}")),
            _ => {
                o.cmd = std::iter::once(a.clone()).chain(it.by_ref().cloned()).collect();
                break;
            }
        }
    }
    if o.cmd.is_empty() {
        return Err("nothing to run: antsy run [flags] -- <command>".into());
    }
    if o.defaults {
        let mut d = strs(&c, "fail_on");
        d.append(&mut o.fail_on);
        o.fail_on = d;
    }
    if o.need_mb == 0 {
        o.need_mb = o.mem_mb;
    }
    if o.name.is_empty() {
        o.name = o.cmd.join(" ").chars().take(40).collect();
    }
    Ok(o)
}

/// Fail fast before starting: not enough memory or no budget left means refuse now, not swap later.
fn admit(o: &Opts, c: &Value, cmdline: &str) -> Result<(), String> {
    let floor = get_f(c, "free_floor_pct") as u64;
    let heavy = config::regexes(&strs(c, "heavy")).iter().any(|r| r.is_match(cmdline));
    if o.need_mb > 0 || heavy {
        let s = mem::sys();
        let after = s.free_mb().saturating_sub(o.need_mb) * 100 / s.total_mb.max(1);
        if s.pressure >= 4 || after < floor {
            return Err(format!(
                "{}; this job needs {} and the floor is {floor}% free.\n{}",
                s.line(),
                fmt_mb(o.need_mb),
                crate::board::render(false)
            ));
        }
    }
    if o.cost_usd > 0.0 {
        let cap = get_f(c, "spend_cap_usd_day");
        let spent = crate::spend::today_total();
        if cap > 0.0 && spent + o.cost_usd > cap {
            return Err(format!(
                "spend cap: ${spent:.2} spent today + ${:.2} > ${cap:.2}/day. Use a cheaper rung (mock, replay, smaller model) or raise spend_cap_usd_day.",
                o.cost_usd
            ));
        }
    }
    Ok(())
}

pub fn main(args: &[String]) -> i32 {
    let o = match parse(args) {
        Ok(o) => o,
        Err(e) => {
            eprintln!("antsy run: {e}");
            return 2;
        }
    };
    let c = config::load();
    let cmdline = o.cmd.join(" ");
    if let Err(why) = admit(&o, &c, &cmdline) {
        eprintln!("antsy: REFUSED {} — {why}", o.name);
        emit("fail", &session(), "", "refused", &format!("{}: {}", o.name, why.lines().next().unwrap_or("")));
        return EXIT_REFUSED;
    }
    daemon::ensure();

    let id = new_id(&o.name);
    let sess = session();
    let log_path = dir("logs").join(format!("{id}.log"));
    let job_path = dir("jobs").join(format!("{id}.json"));
    let stop_path = dir("jobs").join(format!("{id}.stop"));
    let trace_id = format!("{}{}", hex(rand64()), hex(rand64()));
    let span_id = hex(rand64());
    let mut log = std::fs::File::create(&log_path).ok();

    let mut cmd = if o.cmd.len() == 1 {
        let mut c = Command::new("sh");
        c.arg("-c").arg(&o.cmd[0]);
        c
    } else {
        let mut c = Command::new(&o.cmd[0]);
        c.args(&o.cmd[1..]);
        c
    };
    use std::os::unix::process::CommandExt;
    cmd.process_group(0).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped());
    cmd.env("TRACEPARENT", format!("00-{trace_id}-{span_id}-01"));
    cmd.env("ANTSY_JOB", &id);
    if !o.purpose.is_empty() {
        cmd.env("ANTSY_FOR", &o.purpose);
    }
    cmd.env("ANTSY_SESSION", &sess);
    let mut attrs = format!("antsy.job={id},antsy.session={sess}");
    if let Ok(prev) = std::env::var("OTEL_RESOURCE_ATTRIBUTES") {
        attrs = format!("{prev},{attrs}");
    }
    cmd.env("OTEL_RESOURCE_ATTRIBUTES", attrs);
    if std::env::var("OTEL_EXPORTER_OTLP_ENDPOINT").is_err() {
        cmd.env("OTEL_EXPORTER_OTLP_ENDPOINT", format!("http://127.0.0.1:{}", get_str(&c, "otlp_port")));
        cmd.env("OTEL_EXPORTER_OTLP_PROTOCOL", "http/json");
        cmd.env("OTEL_EXPORTER_OTLP_COMPRESSION", "none");
    }
    // Rust builds: fewer parallel rustc jobs when memory is short, instead of swapping.
    if cmdline.contains("cargo ") && std::env::var("CARGO_BUILD_JOBS").is_err() {
        let s = mem::sys();
        let jobs = (s.free_mb() / 1536).clamp(1, mem::ncpu());
        cmd.env("CARGO_BUILD_JOBS", jobs.to_string());
    }

    for sig in [libc::SIGINT, libc::SIGTERM, libc::SIGHUP] {
        unsafe {
            libc::signal(sig, on_signal as extern "C" fn(i32) as libc::sighandler_t);
        }
    }

    let start = now();
    let mut child = match cmd.spawn() {
        Ok(ch) => ch,
        Err(e) => {
            eprintln!("antsy: cannot start {}: {e}", o.cmd[0]);
            emit("fail", &sess, &id, "spawn", &format!("{}: {e}", o.name));
            return 127;
        }
    };
    let pgid = child.id() as i64;
    let mut job = json!({
        "id": id, "name": o.name, "session": sess, "cmd": cmdline,
        "cwd": std::env::current_dir().map(|d| d.display().to_string()).unwrap_or_default(),
        "pid": std::process::id(), "pgid": pgid, "started": start, "status": "running",
        "stall": o.stall, "max": o.max, "mem_mb": o.mem_mb, "log": log_path.display().to_string(),
        "last_output": start, "lines": 0, "rss_mb": 0, "peak_mb": 0, "trace_id": trace_id,
        "for": o.purpose, "cost_usd": o.cost_usd,
    });
    write_json(&job_path, &job);
    emit("start", &sess, &id, "", &o.name);
    if !o.quiet {
        eprintln!("antsy: {id} started (stall {}, log {})", fmt_ago(o.stall), log_path.display());
    }

    let (tx, rx) = mpsc::channel::<(bool, String)>();
    let out = child.stdout.take().unwrap();
    let err = child.stderr.take().unwrap();
    for (is_err, pipe) in [(false, Box::new(out) as Box<dyn std::io::Read + Send>), (true, Box::new(err))] {
        let tx = tx.clone();
        std::thread::spawn(move || {
            let mut r = BufReader::new(pipe);
            let mut buf = vec![];
            while r.read_until(b'\n', &mut buf).map(|n| n > 0).unwrap_or(false) {
                let _ = tx.send((is_err, String::from_utf8_lossy(&buf).trim_end_matches(['\n', '\r']).to_string()));
                buf.clear();
            }
        });
    }
    drop(tx);

    let fails = config::regexes(&o.fail_on);
    let ignores = config::regexes(&strs(&c, "ignore"));
    let mut last_out = Instant::now();
    let mut last_tick = Instant::now() - Duration::from_secs(5);
    let mut progress_mtime = None;
    let mut lines = 0u64;
    let mut peak = 0u64;
    let mut tripped: Option<(&str, String, i32)> = None;
    let mut fail_line: Option<(String, Instant)> = None;
    let mut eof = false;
    let mut exited: Option<(i32, Instant)> = None;
    let begun = Instant::now();

    let code = loop {
        match rx.recv_timeout(Duration::from_millis(200)) {
            Ok((is_err, line)) => {
                last_out = Instant::now();
                lines += 1;
                if let Some(l) = log.as_mut() {
                    let _ = writeln!(l, "{line}");
                }
                if !o.quiet {
                    if is_err {
                        eprintln!("{line}");
                    } else {
                        println!("{line}");
                    }
                }
                if fail_line.is_none()
                    && fails.iter().any(|r| r.is_match(&line))
                    && !ignores.iter().any(|r| r.is_match(&line))
                {
                    fail_line = Some((line, Instant::now()));
                }
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => eof = true,
            Err(mpsc::RecvTimeoutError::Timeout) => {}
        }
        if exited.is_none() {
            if let Ok(Some(st)) = child.try_wait() {
                use std::os::unix::process::ExitStatusExt;
                exited = Some((st.code().unwrap_or_else(|| 128 + st.signal().unwrap_or(0)), Instant::now()));
            }
        }
        // A grandchild can hold the pipes open after the child exits; that must not keep us waiting.
        if let Some((code, at)) = exited {
            if eof || at.elapsed() > Duration::from_secs(1) {
                break code;
            }
        }
        if eof {
            std::thread::sleep(Duration::from_millis(50));
        }
        if tripped.is_some() {
            continue;
        }
        if let Some((l, at)) = &fail_line {
            if at.elapsed().as_secs_f64() >= o.grace {
                tripped = Some(("fail_on", format!("matched: {l}"), EXIT_FAIL_ON));
            }
        }
        let sig = SIGNAL.load(Ordering::SeqCst);
        if sig != 0 {
            tripped = Some(("interrupted", format!("supervisor got signal {sig}"), 128 + sig));
        }
        if last_tick.elapsed() >= Duration::from_secs(1) && tripped.is_none() {
            last_tick = Instant::now();
            if let Some(pf) = &o.progress_file {
                let m = std::fs::metadata(pf).and_then(|m| m.modified()).ok();
                if m.is_some() && m != progress_mtime {
                    progress_mtime = m;
                    last_out = Instant::now();
                }
            }
            let quiet_for = last_out.elapsed().as_secs_f64();
            let rss = mem::group_rss_mb(pgid);
            peak = peak.max(rss);
            if o.stall > 0.0 && quiet_for > o.stall {
                tripped = Some(("stall", format!("no output or progress for {}", fmt_ago(quiet_for)), EXIT_STALL));
            } else if o.max > 0.0 && begun.elapsed().as_secs_f64() > o.max {
                tripped = Some(("max", format!("hit the {} ceiling", fmt_ago(o.max)), EXIT_MAX));
            } else if o.mem_mb > 0 && rss > o.mem_mb {
                tripped = Some(("mem", format!("process group at {} > cap {}", fmt_mb(rss), fmt_mb(o.mem_mb)), EXIT_MEM));
            } else if stop_path.exists() {
                let why = std::fs::read_to_string(&stop_path).unwrap_or_default();
                tripped = Some(("stopped", format!("stop requested: {}", why.trim()), EXIT_STOPPED));
            }
            job["last_output"] = json!(now() - quiet_for);
            job["lines"] = json!(lines);
            job["rss_mb"] = json!(rss);
            job["peak_mb"] = json!(peak);
            write_json(&job_path, &job);
        }
        if let Some((mode, why, _)) = &tripped {
            if !o.quiet {
                eprintln!("antsy: {mode}: {why} — stopping {id}");
            }
            kill_group(pgid, libc::SIGTERM);
            let deadline = Instant::now() + Duration::from_secs(3);
            while Instant::now() < deadline && !matches!(child.try_wait(), Ok(Some(_))) {
                std::thread::sleep(Duration::from_millis(50));
            }
            kill_group(pgid, libc::SIGKILL);
        }
    };
    // Nothing the job started may outlive it.
    kill_group(pgid, libc::SIGKILL);
    let _ = std::fs::remove_file(&stop_path);

    let dur = now() - start;
    let (status, mode, why, exit) = match tripped {
        Some((m, w, e)) => ("failed", m.to_string(), w, e),
        None if code == 0 => ("ok", String::new(), String::new(), 0),
        None => {
            let w = match &fail_line {
                Some((l, _)) => format!("exit {code}; first error: {l}"),
                None => format!("exit {code}"),
            };
            ("failed", "exit".into(), w, code)
        }
    };
    job["status"] = json!(status);
    job["mode"] = json!(mode);
    job["why"] = json!(why);
    job["exit"] = json!(exit);
    job["ended"] = json!(now());
    job["peak_mb"] = json!(peak);
    job["lines"] = json!(lines);
    write_json(&job_path, &job);
    append_line(
        &home().join("history.jsonl"),
        &json!({"id": id, "name": o.name, "for": o.purpose, "cmd": cmdline, "status": status, "mode": mode,
                "secs": dur, "peak_mb": peak, "cost_usd": o.cost_usd, "session": sess, "ts": now()}),
    );
    otel::record_job(&job, &trace_id, &span_id, start, now());

    let summary = if status == "ok" {
        format!("{} in {}, peak {}", o.name, fmt_ago(dur), fmt_mb(peak))
    } else {
        format!("{} ({mode}) {why}; ran {}, peak {}, log {}", o.name, fmt_ago(dur), fmt_mb(peak), log_path.display())
    };
    // A stop the session asked for is an outcome, not a failure to report back to it.
    let kind = match (status, mode.as_str()) {
        ("ok", _) => "ok",
        (_, "stopped") => "done",
        _ => "fail",
    };
    emit(kind, &sess, &id, &mode, &summary);
    eprintln!("antsy {}: {summary}", if status == "ok" { "ok" } else { "FAILED" });
    exit
}
