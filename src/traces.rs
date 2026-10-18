//! Reading traces back cheaply: `antsy traces` lists spans from ~/.antsy/traces, newest last, so an
//! agent can see what a job did (and where it failed) without a tracing UI.
//!
//!   antsy traces [--job ID] [--trace TRACE_ID] [--service S] [--errors] [--slow MS] [--days 1] [-n 30] [--json]

use crate::otel::attr;
use crate::util::*;
use serde_json::{json, Value};

struct Span {
    start_ns: u128,
    ms: f64,
    service: String,
    job: String,
    name: String,
    trace: String,
    error: Option<String>,
}

fn ns(v: &Value) -> u128 {
    match v {
        Value::String(s) => s.parse().unwrap_or(0),
        Value::Number(n) => n.as_u64().unwrap_or(0) as u128,
        _ => 0,
    }
}

fn spans_in(body: &Value, out: &mut Vec<Span>) {
    for r in body["resourceSpans"].as_array().into_iter().flatten() {
        let a = &r["resource"]["attributes"];
        let service = attr(a, "service.name").unwrap_or("unknown").to_string();
        let job = attr(a, "antsy.job").unwrap_or("").to_string();
        for ss in r["scopeSpans"].as_array().into_iter().flatten() {
            for sp in ss["spans"].as_array().into_iter().flatten() {
                let (start, end) = (ns(&sp["startTimeUnixNano"]), ns(&sp["endTimeUnixNano"]));
                let code = &sp["status"]["code"];
                let failed = code.as_i64() == Some(2) || code.as_str() == Some("STATUS_CODE_ERROR");
                out.push(Span {
                    start_ns: start,
                    ms: end.saturating_sub(start) as f64 / 1e6,
                    service: service.clone(),
                    job: job.clone(),
                    name: s(sp, "name").to_string(),
                    trace: s(sp, "traceId").to_string(),
                    error: failed.then(|| s(&sp["status"], "message").to_string()),
                });
            }
        }
    }
}

pub fn main(args: &[String]) -> i32 {
    let opt = |k: &str| args.iter().position(|a| a == k).and_then(|p| args.get(p + 1)).cloned();
    let days: i64 = opt("--days").and_then(|d| d.parse().ok()).unwrap_or(1);
    let n: usize = opt("-n").and_then(|d| d.parse().ok()).unwrap_or(30);
    let slow: f64 = opt("--slow").and_then(|d| d.parse().ok()).unwrap_or(0.0);
    let errors = args.iter().any(|a| a == "--errors");
    let service = opt("--service");
    let job = opt("--job");
    // A job's own span carries its trace id; the program's spans share it through TRACEPARENT.
    let trace = opt("--trace").or_else(|| {
        job.as_ref().and_then(|j| read_json(&dir("jobs").join(format!("{j}.json")))).map(|j| s(&j, "trace_id").to_string())
    });

    let today_n = (now() / 86400.0) as i64;
    let mut spans = vec![];
    for d in (today_n - days + 1)..=today_n {
        let (y, m, dd) = civil(d);
        let p = dir("traces").join(format!("{y:04}-{m:02}-{dd:02}.jsonl"));
        for line in std::fs::read_to_string(p).unwrap_or_default().lines() {
            if let Ok(v) = serde_json::from_str::<Value>(line) {
                if s(&v, "kind") == "traces" {
                    spans_in(&v["body"], &mut spans);
                }
            }
        }
    }
    spans.retain(|sp| {
        (!errors || sp.error.is_some())
            && sp.ms >= slow
            && service.as_ref().map(|x| &sp.service == x).unwrap_or(true)
            && match (&job, &trace) {
                (Some(j), Some(t)) => &sp.job == j || (!t.is_empty() && &sp.trace == t),
                (Some(j), None) => &sp.job == j,
                (None, Some(t)) => &sp.trace == t,
                (None, None) => true,
            }
    });
    spans.sort_by_key(|sp| sp.start_ns);
    let shown = &spans[spans.len().saturating_sub(n)..];
    if args.iter().any(|a| a == "--json") {
        let v: Vec<Value> = shown
            .iter()
            .map(|sp| json!({"start_ns": sp.start_ns.to_string(), "ms": sp.ms, "service": sp.service, "job": sp.job, "name": sp.name, "trace": sp.trace, "error": sp.error}))
            .collect();
        println!("{}", Value::Array(v));
        return 0;
    }
    if shown.is_empty() {
        println!("no matching spans in the last {days} day(s)");
        return 0;
    }
    for sp in shown {
        let ago = now() - sp.start_ns as f64 / 1e9;
        let status = match &sp.error {
            Some(m) if m.is_empty() => "ERROR".to_string(),
            Some(m) => format!("ERROR {m}"),
            None => "ok".to_string(),
        };
        println!(
            "{:>7} ago  {:>9.1}ms  {}  {}  {}{}",
            fmt_ago(ago),
            sp.ms,
            sp.service,
            sp.name,
            status,
            if sp.job.is_empty() { String::new() } else { format!("  [{}]", sp.job) }
        );
    }
    let errs = spans.iter().filter(|s| s.error.is_some()).count();
    println!("{} span(s), {errs} error(s){}", spans.len(), if spans.len() > n { format!("; showing the last {n}") } else { String::new() });
    0
}
