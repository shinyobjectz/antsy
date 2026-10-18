//! OTLP/JSON: spans for antsy's own jobs, and reading what instrumented programs send the daemon.
//! Everything lands in ~/.antsy/traces/<day>.jsonl; a configured `forward` URL gets a copy.

use crate::config::{self, get_str};
use crate::util::*;
use serde_json::{json, Value};
use std::io::Write;
use std::process::{Command, Stdio};

fn kv(k: &str, v: Value) -> Value {
    let val = match v {
        Value::String(s) => json!({"stringValue": s}),
        Value::Number(n) if n.is_i64() => json!({"intValue": n.to_string()}),
        Value::Number(n) => json!({"doubleValue": n}),
        Value::Bool(b) => json!({"boolValue": b}),
        other => json!({"stringValue": other.to_string()}),
    };
    json!({"key": k, "value": val})
}

pub fn attr<'a>(attrs: &'a Value, key: &str) -> Option<&'a str> {
    attrs.as_array()?.iter().find(|a| s(a, "key") == key).and_then(|a| {
        let v = &a["value"];
        v.get("stringValue").and_then(Value::as_str).or_else(|| v.get("intValue").and_then(Value::as_str))
    })
}

pub fn store(kind: &str, body: &Value) {
    append_line(&dir("traces").join(format!("{}.jsonl", today())), &json!({"ts": now(), "kind": kind, "body": body}));
    let fwd = get_str(&config::load(), "forward");
    if !fwd.is_empty() {
        let url = format!("{}/v1/{kind}", fwd.trim_end_matches('/'));
        if let Ok(mut ch) = Command::new("curl")
            .args(["-s", "-m", "3", "-o", "/dev/null", "-H", "content-type: application/json", "--data-binary", "@-", &url])
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
        {
            if let Some(mut i) = ch.stdin.take() {
                let _ = i.write_all(body.to_string().as_bytes());
            }
            std::thread::spawn(move || ch.wait());
        }
    }
}

pub fn record_job(job: &Value, trace_id: &str, span_id: &str, start: f64, end: f64) {
    let ok = s(job, "status") == "ok";
    let ns = |t: f64| ((t * 1e9) as u128).to_string();
    let body = json!({"resourceSpans": [{
        "resource": {"attributes": [
            kv("service.name", json!("antsy")),
            kv("antsy.session", json!(s(job, "session"))),
            kv("antsy.job", json!(s(job, "id"))),
        ]},
        "scopeSpans": [{"scope": {"name": "antsy"}, "spans": [{
            "traceId": trace_id, "spanId": span_id, "name": s(job, "name"), "kind": 1,
            "startTimeUnixNano": ns(start), "endTimeUnixNano": ns(end),
            "attributes": [
                kv("process.command_line", json!(s(job, "cmd"))),
                kv("antsy.mode", json!(s(job, "mode"))),
                kv("antsy.exit", json!(i(job, "exit"))),
                kv("antsy.peak_mb", json!(i(job, "peak_mb"))),
                kv("antsy.lines", json!(i(job, "lines"))),
                kv("antsy.log", json!(s(job, "log"))),
            ],
            "status": if ok { json!({"code": 1}) } else { json!({"code": 2, "message": s(job, "why")}) },
        }]}],
    }]});
    store("traces", &body);
}

/// A failure found in an OTLP batch: (session, job, message).
pub type Failure = (String, String, String);

fn resource_ids(r: &Value) -> (String, String, String) {
    let a = &r["resource"]["attributes"];
    (
        attr(a, "antsy.session").or_else(|| attr(a, "claude.session_id")).unwrap_or("*").to_string(),
        attr(a, "antsy.job").unwrap_or("").to_string(),
        attr(a, "service.name").unwrap_or("unknown").to_string(),
    )
}

/// Error-status spans become failures. Status code 2 (or "STATUS_CODE_ERROR") is the only signal used.
pub fn span_failures(body: &Value) -> Vec<Failure> {
    let mut out = vec![];
    for r in body["resourceSpans"].as_array().into_iter().flatten() {
        let (sess, job, svc) = resource_ids(r);
        for ss in r["scopeSpans"].as_array().into_iter().flatten() {
            for sp in ss["spans"].as_array().into_iter().flatten() {
                let code = &sp["status"]["code"];
                if code.as_i64() == Some(2) || code.as_str() == Some("STATUS_CODE_ERROR") {
                    let m = s(&sp["status"], "message");
                    out.push((sess.clone(), job.clone(), format!("{svc}: span '{}' errored{}{m}", s(sp, "name"), if m.is_empty() { "" } else { ": " })));
                }
            }
        }
    }
    out
}

/// Log records at ERROR or worse (severityNumber >= 17) become failures.
pub fn log_failures(body: &Value) -> Vec<Failure> {
    let mut out = vec![];
    for r in body["resourceLogs"].as_array().into_iter().flatten() {
        let (sess, job, svc) = resource_ids(r);
        for sl in r["scopeLogs"].as_array().into_iter().flatten() {
            for rec in sl["logRecords"].as_array().into_iter().flatten() {
                if i(rec, "severityNumber") >= 17 {
                    let b = rec["body"].get("stringValue").and_then(Value::as_str).unwrap_or("");
                    out.push((sess.clone(), job.clone(), format!("{svc}: {} {}", s(rec, "severityText"), b.chars().take(300).collect::<String>())));
                }
            }
        }
    }
    out
}
