//! `antsy board`: what every agent on this machine is running, measured, not estimated.

use crate::config::{self, get_f, strs};
use crate::util::*;
use crate::{mem, spend};

pub fn running_jobs() -> Vec<serde_json::Value> {
    list("jobs").into_iter().map(|(_, v)| v).filter(|v| s(v, "status") == "running").collect()
}

pub fn watches() -> Vec<serde_json::Value> {
    list("watches").into_iter().map(|(_, v)| v).collect()
}

pub fn render(mine_only: bool) -> String {
    let c = config::load();
    let me = session();
    let sys = mem::sys();
    let mut out = vec![sys.line()];
    if let Some(k) = mem::external_killer(&strs(&c, "external_killers")) {
        out.push(format!("memory killer: {k} (antsy defers to it)"));
    }
    let mut jobs = running_jobs();
    jobs.sort_by_key(|j| std::cmp::Reverse(i(j, "rss_mb")));
    let mut n = 0;
    for j in &jobs {
        let sess = s(j, "session");
        if mine_only && sess != me {
            continue;
        }
        n += 1;
        out.push(format!(
            "  job {} [{}{}] {} — {} now, peak {}, running {}, last output {} ago",
            s(j, "id"),
            short(sess),
            if sess == me { " (you)" } else { "" },
            s(j, "name"),
            fmt_mb(i(j, "rss_mb") as u64),
            fmt_mb(i(j, "peak_mb") as u64),
            fmt_ago(now() - f(j, "started")),
            fmt_ago(now() - f(j, "last_output")),
        ));
    }
    for w in watches() {
        let sess = s(&w, "session");
        if mine_only && sess != me {
            continue;
        }
        n += 1;
        out.push(format!(
            "  watch {} [{}{}] {} — {} for {}",
            s(&w, "id"),
            short(sess),
            if sess == me { " (you)" } else { "" },
            s(&w, "name"),
            s(&w, "kind"),
            fmt_ago(now() - f(&w, "created")),
        ));
    }
    if n == 0 {
        out.push(if mine_only { "  nothing running for this session".into() } else { "  nothing running under antsy".into() });
    }
    let cap = get_f(&c, "spend_cap_usd_day");
    out.push(format!("spend today ${:.2} of ${cap:.2}; daemon {}", spend::today_total(), if crate::daemon::running() { "up" } else { "down" }));
    out.join("\n")
}

pub fn main(args: &[String]) -> i32 {
    println!("{}", render(args.iter().any(|a| a == "--mine")));
    0
}
