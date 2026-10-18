//! `antsy init [--write]`: scan a project once, without tokens, and draft its .antsy.json.
//! The agent reviews the draft (skill: references/setup.md) instead of discovering all this by hand.

use serde_json::{json, Value};
use std::path::{Path, PathBuf};

struct Kind {
    marker: &'static str,
    label: &'static str,
    fail_on: &'static [&'static str],
    heavy: &'static [&'static str],
}

const KINDS: &[Kind] = &[
    Kind { marker: "Cargo.toml", label: "Rust", fail_on: &["^test .* FAILED$", "^error: could not compile"], heavy: &[] },
    Kind {
        marker: "package.json",
        label: "JavaScript/TypeScript",
        fail_on: &["^\\s*●.*›", "Tests?:.*\\d+ failed", "error TS\\d+:", "ERR_PNPM_", "Error: Cannot find module"],
        heavy: &["\\b(next|vite|webpack|turbo|tsc) (build|-b)\\b", "\\bplaywright test\\b"],
    },
    Kind {
        marker: "pyproject.toml",
        label: "Python",
        fail_on: &["^FAILED ", "^E\\s{3}", "^ERROR ", "ModuleNotFoundError"],
        heavy: &["\\bpytest\\b", "\\b(uv|pip) install\\b"],
    },
    Kind { marker: "requirements.txt", label: "Python", fail_on: &["^FAILED ", "^E\\s{3}", "ModuleNotFoundError"], heavy: &["\\bpytest\\b"] },
    Kind { marker: "go.mod", label: "Go", fail_on: &["^--- FAIL:", "^FAIL\\s"], heavy: &[] },
    Kind { marker: "Package.swift", label: "Swift", fail_on: &["error: ", "Test Case .* failed"], heavy: &[] },
    Kind { marker: "project.yml", label: "Xcode (xcodegen)", fail_on: &["Test Case .* failed", "\\*\\* TEST FAILED \\*\\*"], heavy: &["\\bxcodegen\\b"] },
    Kind { marker: "Gemfile", label: "Ruby", fail_on: &["^Failures:", "\\d+ examples?, [1-9]\\d* failures?"], heavy: &["\\brspec\\b"] },
    Kind { marker: "build.gradle", label: "JVM (Gradle)", fail_on: &["FAILURE: Build failed", "BUILD FAILED"], heavy: &["\\bgradlew? (build|test)\\b"] },
    Kind { marker: "pom.xml", label: "JVM (Maven)", fail_on: &["\\[ERROR\\]", "BUILD FAILURE"], heavy: &["\\bmvn (package|test|install)\\b"] },
    Kind { marker: "wrangler.toml", label: "Cloudflare Workers", fail_on: &["✘ \\[ERROR\\]"], heavy: &["\\bwrangler deploy\\b"] },
    Kind { marker: "docker-compose.yml", label: "Docker Compose", fail_on: &["exited with code [1-9]"], heavy: &[] },
];

const PAID_SDKS: &[&str] = &[
    "openai", "anthropic", "@anthropic-ai", "openrouter", "replicate", "@fal-ai", "fal-client", "elevenlabs",
    "google-genai", "@google/genai", "mistralai", "groq", "cohere", "stripe", "twilio", "modal",
];
const OTEL_MARKERS: &[&str] = &["opentelemetry", "@opentelemetry", "go.opentelemetry.io", "tracing-opentelemetry"];
const SERVERS: &[&str] = &["next dev", "vite", "wrangler dev", "uvicorn", "gunicorn", "flask run", "rails s", "cargo watch", "nodemon", "webpack serve", "npm run dev", "pnpm dev", "bun dev"];

/// The project root: the nearest folder with .git or a known manifest.
pub fn root(from: &Path) -> Option<PathBuf> {
    let mut d = from.to_path_buf();
    loop {
        if d.join(".git").exists() || KINDS.iter().any(|k| d.join(k.marker).exists()) {
            return Some(d);
        }
        if !d.pop() {
            return None;
        }
    }
}

pub fn configured(from: &Path) -> bool {
    let mut d = from.to_path_buf();
    loop {
        if d.join(".antsy.json").exists() {
            return true;
        }
        if !d.pop() {
            return false;
        }
    }
}

const SKIP: &[&str] = &["node_modules", "target", ".git", "build", "dist", "vendor", ".venv", "venv", "buck-out", ".next", "Pods", "DerivedData"];

/// Manifests at the root and up to two levels below it (monorepos keep them in packages/ and apps/).
fn find_manifests(root: &Path) -> Vec<(PathBuf, &'static Kind)> {
    let mut out = vec![];
    let mut dirs = vec![(root.to_path_buf(), 0)];
    while let Some((d, depth)) = dirs.pop() {
        for k in KINDS {
            if d.join(k.marker).exists() {
                out.push((d.join(k.marker), k));
            }
        }
        if depth < 2 {
            for e in std::fs::read_dir(&d).into_iter().flatten().flatten() {
                let name = e.file_name().to_string_lossy().into_owned();
                if e.file_type().map(|t| t.is_dir()).unwrap_or(false) && !name.starts_with('.') && !SKIP.contains(&name.as_str()) {
                    dirs.push((e.path(), depth + 1));
                }
            }
        }
    }
    out
}

/// A process's working directory (lsof on macOS, /proc on Linux).
fn cwd_of(pid: &str) -> String {
    if let Ok(p) = std::fs::read_link(format!("/proc/{pid}/cwd")) {
        return p.display().to_string();
    }
    let out = std::process::Command::new("lsof").args(["-a", "-d", "cwd", "-Fn", "-p", pid]).output();
    out.map(|o| String::from_utf8_lossy(&o.stdout).lines().find_map(|l| l.strip_prefix('n').map(String::from)).unwrap_or_default())
        .unwrap_or_default()
}

fn read(p: &Path) -> String {
    std::fs::read_to_string(p).unwrap_or_default().to_lowercase()
}

pub fn main(args: &[String]) -> i32 {
    let cwd = std::env::current_dir().unwrap_or_default();
    let Some(root) = root(&cwd) else {
        eprintln!("antsy init: not inside a project (no .git or manifest above {})", cwd.display());
        return 1;
    };
    let write = args.iter().any(|a| a == "--write");
    let mut fail_on: Vec<String> = crate::config::strs(&crate::config::defaults(), "fail_on");
    let mut heavy: Vec<String> = crate::config::strs(&crate::config::defaults(), "heavy");
    let mut kinds: Vec<(&str, String)> = vec![];
    let mut manifests_text = String::new();
    for (p, k) in find_manifests(&root) {
        let rel = p.parent().and_then(|d| d.strip_prefix(&root).ok()).map(|r| r.display().to_string()).unwrap_or_default();
        kinds.push((k.label, if rel.is_empty() { ".".to_string() } else { format!("{rel}/") }));
        manifests_text.push_str(&read(&p));
        for x in k.fail_on {
            if !fail_on.iter().any(|f| f == x) {
                fail_on.push(x.to_string());
            }
        }
        for x in k.heavy {
            if !heavy.iter().any(|h| h == x) {
                heavy.push(x.to_string());
            }
        }
    }
    let manifests = manifests_text;
    fail_on.dedup();
    heavy.dedup();
    let paid: Vec<&str> = PAID_SDKS.iter().copied().filter(|s| manifests.contains(&format!("\"{s}")) || manifests.contains(&format!("\n{s}"))).collect();
    let otel = OTEL_MARKERS.iter().any(|m| manifests.contains(m));

    let mut labels: Vec<&str> = kinds.iter().map(|k| k.0).collect();
    labels.sort();
    labels.dedup();
    let summary: Vec<String> = labels
        .iter()
        .map(|l| {
            let at: Vec<&str> = kinds.iter().filter(|k| k.0 == *l).map(|k| k.1.as_str()).collect();
            if at.len() > 3 { format!("{l} in {} folders", at.len()) } else { format!("{l} in {}", at.join(" ")) }
        })
        .collect();
    println!("project {} ({})", root.display(), if summary.is_empty() { "unknown kind".to_string() } else { summary.join("; ") });

    // What is already running out of this project: candidates for `antsy watch pid`.
    let ps = std::process::Command::new("ps").args(["-A", "-o", "pid=,command="]).output().map(|o| String::from_utf8_lossy(&o.stdout).into_owned()).unwrap_or_default();
    let roots = root.display().to_string();
    let me = std::process::id().to_string();
    // Paths in the command decide; a generic dev server with no path counts if its cwd is in the project.
    let running: Vec<&str> = ps
        .lines()
        .map(str::trim)
        .filter(|l| !l.starts_with(&me) && !l.contains("antsy "))
        .filter(|l| {
            l.contains(&roots)
                || (SERVERS.iter().any(|s| l.contains(s))
                    && !l.split_whitespace().skip(1).any(|w| w.starts_with('/') && !w.starts_with("/opt") && !w.starts_with("/usr"))
                    && cwd_of(l.split_whitespace().next().unwrap_or("")).starts_with(&roots))
        })
        .take(12)
        .collect();
    if running.is_empty() {
        println!("running: nothing found that looks like this project's processes");
    } else {
        println!("running (attach with `antsy watch pid <pid> --mem <cap>` if it's this project's):");
        for l in running {
            println!("  {}", l.chars().take(140).collect::<String>());
        }
    }
    println!("telemetry: {}", if otel { "OpenTelemetry already a dependency; point its exporter at OTEL_* env (antsy run sets them)" } else { "no OpenTelemetry dependency found; add spans where the project does I/O (references/telemetry.md)" });
    println!("paid SDKs: {}", if paid.is_empty() { "none found".to_string() } else { format!("{} — find or create fakes/recorded fixtures before paid runs (references/cost.md)", paid.join(", ")) });
    let fat: Vec<String> = find_manifests(&root)
        .into_iter()
        .filter(|(p, k)| k.marker == "Cargo.toml" && {
            let t = read(p);
            t.contains("lto = \"fat\"") || t.contains("lto = true") || t.contains("codegen-units = 1")
        })
        .map(|(p, _)| p.strip_prefix(&root).map(|r| r.display().to_string()).unwrap_or_default())
        .collect();
    if !fat.is_empty() {
        println!("rust: fat LTO or codegen-units = 1 in {}; that's the most memory-hungry link (references/memory-and-builds.md)", fat.join(", "));
    }
    let ci = [".github/workflows", ".gitlab-ci.yml", ".circleci"].iter().find(|p| root.join(p).exists());
    if let Some(ci) = ci {
        println!("ci: {ci} — wait on runs with `antsy watch check` (SKILL.md recipes), never by polling yourself");
    }

    let draft: Value = json!({"stall": "120s", "fail_on": fail_on, "heavy": heavy, "ignore": []});
    let path = root.join(".antsy.json");
    if write {
        if path.exists() {
            eprintln!("antsy init: {} exists; not overwriting", path.display());
            return 1;
        }
        let _ = std::fs::write(&path, serde_json::to_string_pretty(&draft).unwrap_or_default() + "\n");
        println!("wrote {}", path.display());
    } else {
        println!("draft .antsy.json (review, then `antsy init --write` or edit and save it yourself):");
        println!("{}", serde_json::to_string_pretty(&draft).unwrap_or_default());
    }
    0
}
