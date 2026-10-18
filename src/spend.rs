//! `antsy spend`: a ledger of money spent testing, with a daily cap that fails fast.

use crate::config::{self, get_f};
use crate::util::*;
use serde_json::json;

fn path() -> std::path::PathBuf {
    home().join("spend.jsonl")
}

pub fn today_total() -> f64 {
    let day = today();
    std::fs::read_to_string(path())
        .unwrap_or_default()
        .lines()
        .filter_map(|l| serde_json::from_str::<serde_json::Value>(l).ok())
        .filter(|v| s(v, "day") == day)
        .map(|v| f(&v, "usd"))
        .sum::<f64>()
        + 0.0
}

pub fn main(args: &[String]) -> i32 {
    let cap = get_f(&config::load(), "spend_cap_usd_day");
    // `--for P` attributes the spend; it defaults to the session's ANTSY_FOR.
    let mut purpose = std::env::var("ANTSY_FOR").unwrap_or_default();
    let mut rest = vec![];
    let mut it = args.iter();
    while let Some(a) = it.next() {
        if a == "--for" || a == "--purpose" {
            purpose = it.next().cloned().unwrap_or_default();
        } else {
            rest.push(a.clone());
        }
    }
    let args = &rest[..];
    if let Some(amount) = args.first() {
        let Ok(usd) = amount.trim_start_matches('$').parse::<f64>() else {
            eprintln!("usage: antsy spend <usd> <what>   |   antsy spend   (today's total)");
            return 2;
        };
        let what = args[1..].join(" ");
        let _ = std::fs::create_dir_all(home());
        append_line(&path(), &json!({"ts": now(), "day": today(), "usd": usd, "what": what, "for": purpose, "session": session()}));
    }
    let total = today_total();
    println!("spent today: ${total:.2} of ${cap:.2}");
    if cap > 0.0 && total > cap {
        eprintln!("antsy: over the daily spend cap; stop paid runs and use a cheaper rung");
        emit("fail", "*", "", "spend", &format!("spent ${total:.2} today, cap ${cap:.2}"));
        return 3;
    }
    0
}
