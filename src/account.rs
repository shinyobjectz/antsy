//! Attribution and allocation: where compute time and money actually went, by purpose, and
//! whether it bought anything. Everything here is measured after the fact; nothing is enforced.
//!
//!   antsy note <job-id|purpose> <what it taught us> [--useful|--waste]
//!   antsy account [--days 7] [--by for|name|mode|session] [--json]

use crate::util::*;
use serde_json::{json, Value};
use std::collections::BTreeMap;

const UNATTRIBUTED: &str = "(unattributed)";

fn rows(file: &str, since: f64) -> Vec<Value> {
    std::fs::read_to_string(home().join(file))
        .unwrap_or_default()
        .lines()
        .filter_map(|l| serde_json::from_str::<Value>(l).ok())
        .filter(|v| f(v, "ts") >= since)
        .collect()
}

pub fn note(args: &[String]) -> i32 {
    let verdict = if args.iter().any(|a| a == "--waste") {
        "waste"
    } else if args.iter().any(|a| a == "--useful") {
        "useful"
    } else {
        "noted"
    };
    let words: Vec<&String> = args.iter().filter(|a| !a.starts_with("--")).collect();
    let Some((reference, text)) = words.split_first() else {
        eprintln!("usage: antsy note <job-id|purpose> <what it taught us> [--useful|--waste]");
        return 2;
    };
    // A job id resolves to that job's purpose, so outcomes add up per purpose.
    let job = read_json(&dir("jobs").join(format!("{reference}.json")));
    let purpose = match &job {
        Some(j) if !s(j, "for").is_empty() => s(j, "for").to_string(),
        Some(_) => UNATTRIBUTED.to_string(),
        None => reference.to_string(),
    };
    let text: Vec<&str> = text.iter().map(|s| s.as_str()).collect();
    append_line(
        &home().join("outcomes.jsonl"),
        &json!({"ts": now(), "ref": reference, "for": purpose, "verdict": verdict, "text": text.join(" "), "session": session()}),
    );
    println!("noted ({verdict}) for {purpose}");
    0
}

#[derive(Default)]
struct Row {
    runs: u64,
    failed: u64,
    secs: f64,
    peak_mb: u64,
    usd: f64,
    useful: u64,
    waste: u64,
    notes: u64,
}

fn key(v: &Value, by: &str) -> String {
    let k = s(v, by);
    if k.is_empty() {
        if by == "for" { UNATTRIBUTED.into() } else { "(none)".into() }
    } else {
        k.to_string()
    }
}

struct Report {
    rows: BTreeMap<String, Row>,
    total: Row,
    loops: Vec<String>,
    silent: Vec<String>,
}

fn report(days: f64, by: &str) -> Report {
    let since = now() - days * 86400.0;
    let hist = rows("history.jsonl", since);
    let mut out: BTreeMap<String, Row> = BTreeMap::new();
    let mut total = Row::default();
    for h in &hist {
        let r = out.entry(key(h, by)).or_default();
        let failed = s(h, "status") != "ok" && s(h, "mode") != "stopped";
        for r in [r, &mut total] {
            r.runs += 1;
            r.failed += failed as u64;
            r.secs += f(h, "secs");
            r.peak_mb = r.peak_mb.max(i(h, "peak_mb") as u64);
        }
    }
    for sp in rows("spend.jsonl", since) {
        let k = if by == "for" { key(&sp, "for") } else { "(spend)".into() };
        out.entry(k).or_default().usd += f(&sp, "usd");
        total.usd += f(&sp, "usd");
    }
    for o in rows("outcomes.jsonl", since) {
        if by != "for" {
            break;
        }
        let r = out.entry(key(&o, "for")).or_default();
        r.notes += 1;
        match s(&o, "verdict") {
            "useful" => r.useful += 1,
            "waste" => r.waste += 1,
            _ => {}
        }
    }
    // The same job failing the same way again and again, with no success since, is a loop, not progress.
    let mut streak: BTreeMap<String, (String, u64)> = BTreeMap::new();
    for h in &hist {
        let e = streak.entry(s(h, "name").to_string()).or_insert((String::new(), 0));
        if s(h, "status") == "ok" {
            *e = (String::new(), 0);
        } else if e.0 == s(h, "mode") {
            e.1 += 1;
        } else {
            *e = (s(h, "mode").to_string(), 1);
        }
    }
    let loops = streak.into_iter().filter(|(_, (_, n))| *n >= 3).map(|(name, (mode, n))| format!("{name} failed {n}× in a row ({mode})")).collect();
    let silent = if by == "for" {
        out.iter()
            .filter(|(k, r)| k.as_str() != UNATTRIBUTED && r.notes == 0 && (r.secs >= 600.0 || r.usd >= 1.0))
            .map(|(k, _)| k.clone())
            .collect()
    } else {
        vec![]
    };
    Report { rows: out, total, loops, silent }
}

fn unattributed_share(r: &Report) -> (f64, f64) {
    let u = r.rows.get(UNATTRIBUTED);
    let pct = |part: f64, all: f64| if all > 0.0 { 100.0 * part / all } else { 0.0 };
    (pct(u.map(|x| x.secs).unwrap_or(0.0), r.total.secs), pct(u.map(|x| x.usd).unwrap_or(0.0), r.total.usd))
}

/// One line for session start, or None when there's no history.
pub fn summary_line() -> Option<String> {
    let r = report(7.0, "for");
    if r.total.runs == 0 && r.total.usd == 0.0 {
        return None;
    }
    let (u_secs, u_usd) = unattributed_share(&r);
    let top = r
        .rows
        .iter()
        .filter(|(k, _)| k.as_str() != UNATTRIBUTED)
        .max_by(|a, b| (a.1.secs + a.1.usd * 600.0).total_cmp(&(b.1.secs + b.1.usd * 600.0)));
    let mut line = format!(
        "Last 7 days under antsy: {} runs ({} failed), {} compute, ${:.2} spent; unattributed {:.0}% of compute, {:.0}% of spend",
        r.total.runs,
        r.total.failed,
        fmt_ago(r.total.secs),
        r.total.usd,
        u_secs,
        u_usd
    );
    if let Some((k, v)) = top {
        line.push_str(&format!("; biggest purpose \"{k}\" ({}, ${:.2})", fmt_ago(v.secs), v.usd));
    }
    if !r.loops.is_empty() {
        line.push_str(&format!("; {} failure loop(s)", r.loops.len()));
    }
    line.push_str(". `antsy account` for the breakdown.");
    Some(line)
}

pub fn main(args: &[String]) -> i32 {
    let days = args.iter().position(|a| a == "--days").and_then(|p| args.get(p + 1)).and_then(|x| x.parse().ok()).unwrap_or(7.0);
    let by = args.iter().position(|a| a == "--by").and_then(|p| args.get(p + 1)).map(String::as_str).unwrap_or("for");
    let r = report(days, by);
    if args.iter().any(|a| a == "--json") {
        let rows: Vec<Value> = r
            .rows
            .iter()
            .map(|(k, v)| json!({by: k, "runs": v.runs, "failed": v.failed, "secs": v.secs, "peak_mb": v.peak_mb, "usd": v.usd, "useful": v.useful, "waste": v.waste, "notes": v.notes}))
            .collect();
        println!("{}", json!({"days": days, "by": by, "rows": rows, "loops": r.loops, "no_outcome": r.silent}));
        return 0;
    }
    let mut sorted: Vec<(&String, &Row)> = r.rows.iter().collect();
    sorted.sort_by(|a, b| (b.1.secs + b.1.usd * 600.0).total_cmp(&(a.1.secs + a.1.usd * 600.0)));
    println!("last {days} days by {by}: {} runs, {} failed, {} compute, ${:.2}", r.total.runs, r.total.failed, fmt_ago(r.total.secs), r.total.usd);
    println!("{:<36} {:>5} {:>6} {:>9} {:>9} {:>8} {:>10}", by, "runs", "fail%", "compute", "peak", "spend", "useful/waste");
    for (k, v) in sorted {
        let fail = (100 * v.failed).checked_div(v.runs).unwrap_or(0);
        println!(
            "{:<36} {:>5} {:>5}% {:>9} {:>9} {:>8} {:>10}",
            k.chars().take(36).collect::<String>(),
            v.runs,
            fail,
            fmt_ago(v.secs),
            fmt_mb(v.peak_mb),
            format!("${:.2}", v.usd),
            format!("{}/{}", v.useful, v.waste)
        );
    }
    if by == "for" {
        let (u_secs, u_usd) = unattributed_share(&r);
        println!("unattributed: {u_secs:.0}% of compute, {u_usd:.0}% of spend");
    }
    for l in &r.loops {
        println!("loop: {l} — change the approach before running it again");
    }
    for s in &r.silent {
        println!("no outcome noted: \"{s}\" — what did it buy? `antsy note \"{s}\" <finding> --useful|--waste`");
    }
    0
}

/// Measured peaks of a job name's recent successful runs, newest last.
pub fn peaks(name: &str, n: usize) -> Vec<u64> {
    let all: Vec<u64> = rows("history.jsonl", 0.0)
        .iter()
        .filter(|h| s(h, "name") == name && s(h, "status") == "ok")
        .map(|h| i(h, "peak_mb") as u64)
        .collect();
    all[all.len().saturating_sub(n)..].to_vec()
}

/// `antsy history [NAME] [-n 15]`: what past runs measured. Data for the next contract, not a forecast.
pub fn history(args: &[String]) -> i32 {
    let n: usize = args.iter().position(|a| a == "-n").and_then(|p| args.get(p + 1)).and_then(|x| x.parse().ok()).unwrap_or(15);
    let name = args.iter().enumerate().find(|(k, a)| !a.starts_with('-') && (*k == 0 || args[k - 1] != "-n")).map(|(_, a)| a.as_str());
    let all = rows("history.jsonl", 0.0);
    let hits: Vec<&Value> = all.iter().filter(|h| name.map(|x| s(h, "name") == x).unwrap_or(true)).collect();
    if hits.is_empty() {
        println!("no runs recorded{}", name.map(|x| format!(" for {x}")).unwrap_or_default());
        return 0;
    }
    for h in &hits[hits.len().saturating_sub(n)..] {
        let outcome = if s(h, "status") == "ok" { "ok".to_string() } else { format!("FAILED {}", s(h, "mode")) };
        println!(
            "{:>7} ago  {:<28} {:<16} ran {:>7}  peak {:>8}{}",
            fmt_ago(now() - f(h, "ts")),
            s(h, "name").chars().take(28).collect::<String>(),
            outcome,
            fmt_ago(f(h, "secs")),
            fmt_mb(i(h, "peak_mb") as u64),
            if s(h, "for").is_empty() { String::new() } else { format!("  for: {}", s(h, "for")) }
        );
    }
    if let Some(x) = name {
        let p = peaks(x, 5);
        if let Some(max) = p.iter().max() {
            println!("highest peak of the last {} ok run(s): {}; `--mem auto` would cap at {}", p.len(), fmt_mb(*max), fmt_mb(auto_cap(*max)));
        }
    }
    0
}

/// The cap `--mem auto` uses: 30% over the highest recent peak, at least 256 MB.
pub fn auto_cap(peak: u64) -> u64 {
    (peak + peak * 3 / 10).max(256)
}
