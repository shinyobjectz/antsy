//! `antsy hook <event>`: Claude Code hooks. They read state files only, so they cost
//! milliseconds and no tokens unless there is something to say.

use crate::config::{self, get_f, get_str, strs};
use crate::util::*;
use crate::{board, feed, mem, spend};
use serde_json::{json, Value};
use std::io::Read;

pub const CREED: &str = "Antsy is on (fail fast, never wait on a clock):
- Before starting anything that runs longer than a moment, decide how it fails: liveness (`--stall`), memory (`--mem`), cost, and the lines that mean failure. Run it as `antsy run --name N [--stall 2m] [--mem 8G] -- <cmd>` with run_in_background, or `antsy watch pid|log|check` for things you didn't start.
- Don't estimate durations or plan by the clock, and never `sleep` to wait. Wait on events: a background `antsy wait`, or the Monitor tool on `antsy events --follow`. Report what you observe (running, last output, peak memory), not ETAs.
- Leave nothing hanging: every job has a waiter or gets stopped (`antsy stop`) before you finish. `antsy board` shows what every agent on this machine is running.
- Test cheapest first: offline, mocks, recorded replays and tiny inputs before any paid call; log real spend with `antsy spend <usd> <what>`.
- Memory is shared: check `antsy board` before heavy local work (builds, models). antsy refuses heavy jobs below the free-memory floor instead of letting the machine swap.
- Attribute and account: say what each run or dollar is for (`--for '<question or goal>'`, or export ANTSY_FOR), note what it bought (`antsy note`), and let `antsy account` show whether time and money are going where they should. For ML, evals, tests and automations, know your data: where it came from, its slices, and which results each slice explains.
Skill: antsy (philosophy and recipes).";

fn input() -> Value {
    let mut s = String::new();
    let _ = std::io::stdin().read_to_string(&mut s);
    serde_json::from_str(&s).unwrap_or(Value::Null)
}

fn context(event: &str, text: String) {
    println!("{}", json!({"hookSpecificOutput": {"hookEventName": event, "additionalContext": text}}));
}

fn deny(reason: String) {
    println!(
        "{}",
        json!({"hookSpecificOutput": {"hookEventName": "PreToolUse", "permissionDecision": "deny", "permissionDecisionReason": reason}})
    );
}

/// Nudges are said once per session per kind (config `nudge`: once | always | off).
fn nudge_ok(c: &Value, sess: &str, kind: &str) -> bool {
    match get_str(c, "nudge").as_str() {
        "off" => false,
        "always" => true,
        _ => {
            let p = dir("nudged").join(format!("{}-{kind}", sess.replace('/', "_")));
            if p.exists() {
                false
            } else {
                let _ = std::fs::write(p, "");
                true
            }
        }
    }
}

pub fn main(args: &[String]) -> i32 {
    let ev = args.first().map(String::as_str).unwrap_or("");
    let inp = input();
    let sess = match s(&inp, "session_id") {
        "" => session(),
        x => x.to_string(),
    };
    match ev {
        "session-start" => session_start(&sess, s(&inp, "cwd")),
        "pre-bash" => pre_bash(&sess, s(&inp["tool_input"], "command")),
        "post-tool" => deliver(&sess, "PostToolUse"),
        "prompt" => deliver(&sess, "UserPromptSubmit"),
        "stop" => stop(&sess, inp["stop_hook_active"].as_bool().unwrap_or(false)),
        "session-end" => session_end(&sess),
        _ => {}
    }
    0
}

fn session_start(sess: &str, cwd: &str) {
    crate::daemon::ensure();
    feed::start_cursor(sess);
    let mut text = CREED.to_string();
    let cwd = if cwd.is_empty() { std::env::current_dir().unwrap_or_default() } else { cwd.into() };
    let home = std::path::PathBuf::from(std::env::var("HOME").unwrap_or_default());
    if let Some(root) = crate::init::root(&cwd).filter(|r| *r != home) {
        if !crate::init::configured(&cwd) {
            text.push_str(&format!(
                "\n\nThis project ({}) has no .antsy.json yet. Before its first long-running work, set it up: \
                 follow the antsy skill's references/setup.md (start with `antsy init`). A `{{}}` .antsy.json opts out.",
                root.display()
            ));
        }
    }
    let (jobs, ws) = feed::mine_running(sess);
    if !jobs.is_empty() || !ws.is_empty() {
        text.push_str("\n\nStill running for this session:\n");
        text.push_str(&board::render(true));
    }
    if let Some(line) = crate::account::summary_line() {
        text.push_str("\n\n");
        text.push_str(&line);
    }
    let others = board::running_jobs().len() - jobs.len();
    if others > 0 {
        text.push_str(&format!("\n{others} job(s) from other sessions are running; `antsy board` before heavy work."));
    }
    context("SessionStart", text);
}

fn deliver(sess: &str, event: &str) {
    let lines = feed::undelivered(sess, 8);
    if !lines.is_empty() {
        context(event, lines.join("\n"));
    }
}

fn sleep_secs(cmd: &str) -> f64 {
    let re = regex_lite::Regex::new(r"(?:^|[;&|(\s])sleep\s+([0-9.]+)\s*([smhd]?)").unwrap();
    re.captures_iter(cmd)
        .map(|c| parse_dur(&format!("{}{}", &c[1], c.get(2).map(|m| m.as_str()).unwrap_or(""))))
        .fold(0.0, f64::max)
}

fn pre_bash(sess: &str, cmd: &str) {
    if cmd.is_empty() {
        return;
    }
    let c = config::load();
    let wrapped = cmd.contains("antsy run") || cmd.contains("antsy watch");
    let sleep_max = get_f(&c, "sleep_max_s");
    let slept = sleep_secs(cmd);
    let poll_loop = regex_lite::Regex::new(r"\b(while|until)\b[\s\S]*\bsleep\b").unwrap().is_match(cmd);
    if !wrapped && (slept > sleep_max || poll_loop) {
        deny(format!(
            "antsy: don't wait on a clock{}. Wait on an event instead: run the work with run_in_background (you're notified when it exits), \
             `antsy watch check --until-ok -- <probe>` plus a background `antsy wait`, or the Monitor tool on `antsy events --follow`. \
             If you truly need a short poll, bound it: `antsy run --max 30s -- sh -c 'until <probe>; do sleep 1; done'`.",
            if poll_loop { " (unbounded poll loop)" } else { "" }
        ));
        return;
    }
    if wrapped {
        return;
    }
    let heavy = config::regexes(&strs(&c, "heavy")).iter().any(|r| r.is_match(cmd));
    if heavy {
        let sys = mem::sys();
        let floor = get_f(&c, "free_floor_pct") as u64;
        if sys.pressure >= 4 || sys.free_pct < floor {
            deny(format!(
                "antsy: refusing heavy work now: {} (floor {floor}%). Don't queue behind it with sleeps; look at who holds memory and decide:\n{}",
                sys.line(),
                board::render(false)
            ));
            return;
        }
        if nudge_ok(&c, sess, "heavy") {
            context(
                "PreToolUse",
                "antsy: heavy command. Prefer `antsy run --name <n> --for \"<what it's for>\" [--mem <cap>] -- <cmd>` (stall/memory/fail-fast, \
                 trace, attribution, event to this session) with run_in_background; it also sizes CARGO_BUILD_JOBS to free memory."
                    .into(),
            );
        }
    }
    for p in c.get("paid").and_then(Value::as_array).into_iter().flatten() {
        let Ok(re) = regex_lite::Regex::new(s(p, "match")) else { continue };
        if !re.is_match(cmd) {
            continue;
        }
        let cap = get_f(&c, "spend_cap_usd_day");
        let spent = spend::today_total();
        if cap > 0.0 && spent >= cap {
            deny(format!("antsy: daily spend cap reached (${spent:.2} of ${cap:.2}). Use a free rung (mock, recorded replay, local model) or ask the user to raise spend_cap_usd_day."));
            return;
        }
        if nudge_ok(&c, sess, "paid") {
            context(
                "PreToolUse",
                format!(
                    "antsy: this looks like a paid call ({}). Cheapest rung first: is there a mock, a recorded response, a smaller model, \
                     a 1-item input, or a dry-run flag? Cache what you get. Log real spend with its purpose: `antsy spend <usd> <what> --for \"<goal>\"` (today ${spent:.2} of ${cap:.2}).",
                    s(p, "note")
                ),
            );
        }
        return;
    }
}

fn stop(sess: &str, already: bool) {
    let undelivered = feed::undelivered(sess, 8);
    let fails: Vec<&String> = undelivered.iter().filter(|l| l.contains("FAILED")).collect();
    if !fails.is_empty() && !already {
        let list: Vec<&str> = fails.iter().map(|s| s.as_str()).collect();
        block(format!("antsy: failures you haven't looked at:\n{}", list.join("\n")));
        return;
    }
    if already {
        return;
    }
    let (jobs, ws) = feed::mine_running(sess);
    // Background jobs launched by Claude Code's own tools notify on exit; detached ones don't.
    let unwatched: Vec<String> = jobs
        .iter()
        .filter(|j| ppid_of(i(j, "pid")) <= 1)
        .map(|j| format!("job {} ({})", s(j, "id"), s(j, "name")))
        .chain(ws.iter().map(|w| format!("watch {} ({})", s(w, "id"), s(w, "name"))))
        .collect();
    if !unwatched.is_empty() && !feed::has_waiter(sess) {
        block(format!(
            "antsy: still running with nobody listening: {}. Before stopping, either start a waiter (Bash run_in_background: `antsy wait`), \
             watch with Monitor on `antsy events --follow`, or `antsy stop <id>`.",
            unwatched.join(", ")
        ));
    }
}

fn block(reason: String) {
    println!("{}", json!({"decision": "block", "reason": reason}));
}

/// Nobody will hear this session's watches or detached jobs after it ends, so they end with it
/// (config `on_session_end`: "stop", the default, or "keep").
fn session_end(sess: &str) {
    if get_str(&config::load(), "on_session_end") == "keep" {
        return;
    }
    let (jobs, ws) = feed::mine_running(sess);
    for w in ws {
        let _ = std::fs::remove_file(dir("watches").join(format!("{}.json", s(&w, "id"))));
        emit("done", sess, s(&w, "id"), "session_end", &format!("{}: watch removed when its session ended", s(&w, "name")));
    }
    for j in jobs.iter().filter(|j| ppid_of(i(j, "pid")) <= 1) {
        let _ = std::fs::write(dir("jobs").join(format!("{}.stop", s(j, "id"))), "its session ended");
    }
    let _ = std::fs::remove_file(dir("cursors").join(format!("{}.json", sess.replace('/', "_"))));
}
