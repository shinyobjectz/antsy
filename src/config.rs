//! Defaults, overridden by ~/.antsy/config.json, overridden by the nearest .antsy.json above the cwd.

use crate::util::{home, read_json};
use serde_json::{json, Value};

pub fn defaults() -> Value {
    json!({
        // Liveness, not deadlines: a job that prints nothing (and touches no progress file) this long is hung.
        "stall": "120s",
        // A ceiling, never a plan. It only catches things that are alive but never finish.
        "max": "2h",
        // Lines that mean the run has already failed. The first one stops the job after `grace`.
        "fail_on": [
            "^\\s*error(\\[E\\d+\\])?:",
            "^\\s*(fatal|FATAL|panic|PANIC)\\b",
            "thread '.*' panicked",
            "Traceback \\(most recent call last\\)",
            "(?i)out of memory|OOMKilled|Killed: 9|Segmentation fault",
            "(?i)^\\s*npm ERR!",
            "\\*\\* BUILD FAILED \\*\\*"
        ],
        "ignore": [],
        "grace": "3s",
        // Refuse heavy work when less memory than this is free, instead of swapping.
        "free_floor_pct": 15,
        // "auto": antsy kills its own jobs at the memory edge only when no external killer is running.
        "mem_killer": "auto",
        "external_killers": ["memguard", "earlyoom", "systemd-oomd", "nohang"],
        "sleep_max_s": 5,
        "heavy": [
            "\\bcargo (build|test|run|bench|clippy|install)\\b",
            "\\bxcodebuild\\b",
            "\\bswift (build|test)\\b",
            "\\b(npm|pnpm|yarn|bun) (run )?(build|test)\\b",
            "\\bdocker (build|compose up|run)\\b",
            "\\bgo (build|test)\\b",
            "\\b(ollama (run|serve)|llama-server|llama-cli|mlx_lm|vllm|torchrun)\\b",
            "\\bffmpeg\\b"
        ],
        "paid": [
            {"match": "api\\.openai\\.com|api\\.anthropic\\.com|openrouter\\.ai|generativelanguage\\.googleapis\\.com|api\\.mistral\\.ai|api\\.groq\\.com|api\\.together",
             "note": "LLM API"},
            {"match": "replicate\\.com|fal\\.(ai|run)|elevenlabs|api\\.runwayml|modal (run|deploy)|runpod",
             "note": "paid generation / GPU"},
            {"match": "\\b(aws|gcloud|az) .*(create|run|deploy)\\b",
             "note": "cloud resources"}
        ],
        "spend_cap_usd_day": 10.0,
        "otlp_port": 4318,
        "idle_exit": "60m",
        // "once" per session per kind, "always", or "off".
        "nudge": "once"
    })
}

fn merge(base: &mut Value, over: Value) {
    if let (Some(b), Value::Object(o)) = (base.as_object_mut(), over) {
        for (k, v) in o {
            b.insert(k, v);
        }
    }
}

pub fn load() -> Value {
    let mut c = defaults();
    if let Some(v) = read_json(&home().join("config.json")) {
        merge(&mut c, v);
    }
    if let Ok(mut d) = std::env::current_dir() {
        loop {
            if let Some(v) = read_json(&d.join(".antsy.json")) {
                merge(&mut c, v);
                break;
            }
            if !d.pop() {
                break;
            }
        }
    }
    c
}

pub fn strs(c: &Value, k: &str) -> Vec<String> {
    c.get(k)
        .and_then(Value::as_array)
        .map(|a| a.iter().filter_map(|x| x.as_str().map(String::from)).collect())
        .unwrap_or_default()
}

pub fn regexes(pats: &[String]) -> Vec<regex_lite::Regex> {
    pats.iter().filter_map(|p| regex_lite::Regex::new(p).ok()).collect()
}

pub fn get_str(c: &Value, k: &str) -> String {
    match c.get(k) {
        Some(Value::String(s)) => s.clone(),
        Some(Value::Number(n)) => n.to_string(),
        _ => String::new(),
    }
}

pub fn get_f(c: &Value, k: &str) -> f64 {
    c.get(k).and_then(Value::as_f64).unwrap_or(0.0)
}
