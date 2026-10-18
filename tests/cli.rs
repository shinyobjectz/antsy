//! End-to-end checks of the failure modes, run against the built binary with a private ANTSY_HOME.

use std::process::{Command, Output};
use std::time::Instant;

struct Env {
    home: std::path::PathBuf,
}

impl Env {
    fn new(name: &str) -> Env {
        let home = std::env::temp_dir().join(format!("antsy-test-{name}-{}", std::process::id()));
        std::fs::create_dir_all(&home).unwrap();
        // Port 0 keeps tests off the real daemon's port; ANTSY_NO_DAEMON keeps them self-contained.
        std::fs::write(home.join("config.json"), r#"{"otlp_port": 0}"#).unwrap();
        Env { home }
    }
    fn antsy(&self, args: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_antsy"))
            .args(args)
            .env("ANTSY_HOME", &self.home)
            .env("ANTSY_NO_DAEMON", "1")
            .env("ANTSY_SESSION", "test")
            .env_remove("CLAUDE_CODE_SESSION_ID")
            .env_remove("ANTSY_FOR")
            .output()
            .unwrap()
    }
    fn events(&self) -> String {
        std::fs::read_to_string(self.home.join("events.jsonl")).unwrap_or_default()
    }
}

impl Drop for Env {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.home);
    }
}

#[test]
fn ok_passes_through() {
    let e = Env::new("ok");
    let o = e.antsy(&["run", "-q", "--", "echo", "hi"]);
    assert_eq!(o.status.code(), Some(0));
    assert!(e.events().contains("\"kind\":\"ok\""));
}

#[test]
fn child_exit_code_is_kept() {
    let e = Env::new("exit");
    let o = e.antsy(&["run", "-q", "--", "sh -c 'exit 7'"]);
    assert_eq!(o.status.code(), Some(7));
}

#[test]
fn fail_on_stops_early() {
    let e = Env::new("failon");
    let t = Instant::now();
    let o = e.antsy(&["run", "-q", "--grace", "0.1s", "--", "sh -c 'echo \"error: boom\"; sleep 20'"]);
    assert_eq!(o.status.code(), Some(123));
    assert!(t.elapsed().as_secs() < 8);
    assert!(e.events().contains("error: boom"));
}

#[test]
fn stall_stops_a_silent_job() {
    let e = Env::new("stall");
    let t = Instant::now();
    let o = e.antsy(&["run", "-q", "--stall", "1s", "--", "sh -c 'echo start; sleep 20'"]);
    assert_eq!(o.status.code(), Some(121));
    assert!(t.elapsed().as_secs() < 8);
}

#[test]
fn max_is_a_ceiling() {
    let e = Env::new("max");
    let o = e.antsy(&["run", "-q", "--stall", "0", "--max", "1s", "--", "sh -c 'while :; do echo x; sleep 0.2; done'"]);
    assert_eq!(o.status.code(), Some(124));
}

#[test]
fn grandchildren_do_not_hold_the_supervisor() {
    let e = Env::new("grandchild");
    let t = Instant::now();
    let o = e.antsy(&["run", "-q", "--", "sh -c '(sleep 20 &); echo done'"]);
    assert_eq!(o.status.code(), Some(0));
    assert!(t.elapsed().as_secs() < 5);
}

#[test]
fn admission_refuses_impossible_memory() {
    let e = Env::new("refuse");
    let o = e.antsy(&["run", "-q", "--need-mem", "100000G", "--", "true"]);
    assert_eq!(o.status.code(), Some(125));
}

#[test]
fn spend_cap_fails_fast() {
    let e = Env::new("spend");
    std::fs::write(e.home.join("config.json"), r#"{"otlp_port": 0, "spend_cap_usd_day": 1.0}"#).unwrap();
    assert_eq!(e.antsy(&["spend", "0.60", "a"]).status.code(), Some(0));
    assert_eq!(e.antsy(&["run", "-q", "--cost-usd", "0.50", "--", "true"]).status.code(), Some(125));
    assert_eq!(e.antsy(&["spend", "0.60", "b"]).status.code(), Some(3));
}

#[test]
fn wait_never_waits_on_nothing() {
    let e = Env::new("wait");
    let t = Instant::now();
    assert_eq!(e.antsy(&["wait"]).status.code(), Some(0));
    assert!(t.elapsed().as_secs() < 3);
}

#[test]
fn pre_bash_denies_long_sleeps_and_poll_loops() {
    let e = Env::new("hook");
    let hook = |cmd: &str| {
        let mut c = Command::new(env!("CARGO_BIN_EXE_antsy"))
            .args(["hook", "pre-bash"])
            .env("ANTSY_HOME", &e.home)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        use std::io::Write;
        let body = serde_json::json!({"session_id": "t", "tool_input": {"command": cmd}});
        c.stdin.take().unwrap().write_all(body.to_string().as_bytes()).unwrap();
        String::from_utf8(c.wait_with_output().unwrap().stdout).unwrap()
    };
    assert!(hook("sleep 60 && ls").contains("deny"));
    assert!(hook("until curl -s x; do sleep 1; done").contains("deny"));
    assert!(!hook("sleep 1").contains("deny"));
    assert!(!hook("antsy run --max 30s -- sh -c 'until x; do sleep 1; done'").contains("deny"));
}

#[test]
fn init_drafts_config_for_a_project() {
    let e = Env::new("init");
    let proj = e.home.join("proj");
    std::fs::create_dir_all(proj.join("crates/a")).unwrap();
    std::fs::write(proj.join("crates/a/Cargo.toml"), "[package]\nname = \"a\"\n[profile.release]\nlto = \"fat\"\n").unwrap();
    std::fs::write(proj.join("package.json"), r#"{"dependencies": {"openai": "4"}}"#).unwrap();
    let run = |args: &[&str]| {
        Command::new(env!("CARGO_BIN_EXE_antsy")).args(args).current_dir(&proj).env("ANTSY_HOME", &e.home).output().unwrap()
    };
    let out = String::from_utf8(run(&["init"]).stdout).unwrap();
    assert!(out.contains("Rust in crates/a/"), "{out}");
    assert!(out.contains("openai"), "{out}");
    assert!(out.contains("fat LTO"), "{out}");
    assert_eq!(run(&["init", "--write"]).status.code(), Some(0));
    assert!(proj.join(".antsy.json").exists());
    assert_eq!(run(&["init", "--write"]).status.code(), Some(1), "never overwrites");
}

#[test]
fn account_attributes_runs_spend_and_loops() {
    let e = Env::new("account");
    for _ in 0..3 {
        e.antsy(&["run", "-q", "--name", "flaky", "--for", "fix login test", "--", "sh -c 'exit 1'"]);
    }
    e.antsy(&["run", "-q", "--name", "misc", "--", "true"]);
    e.antsy(&["spend", "0.25", "judge", "--for", "fix login test"]);
    assert_eq!(e.antsy(&["note", "fix login test", "the fixture was stale", "--useful"]).status.code(), Some(0));
    let out = String::from_utf8(e.antsy(&["account"]).stdout).unwrap();
    assert!(out.contains("fix login test"), "{out}");
    assert!(out.contains("$0.25"), "{out}");
    assert!(out.contains("(unattributed)"), "{out}");
    assert!(out.contains("loop: flaky failed 3×"), "{out}");
}
