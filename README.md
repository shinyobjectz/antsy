# Antsy

[![ci](https://github.com/shinyobjectz/antsy/actions/workflows/ci.yml/badge.svg)](https://github.com/shinyobjectz/antsy/actions/workflows/ci.yml)
[![license](https://img.shields.io/badge/license-Apache--2.0-blue.svg)](LICENSE)

**Fail fast. Never wait on a clock.** Antsy is a Claude Code plugin, skill, and small Rust binary that
makes a coding agent impatient with failure:

- **Failure-mode contracts.** Every long job declares how it fails *before* it starts: liveness
  (`--stall`), memory (`--mem`), cost, and the lines that mean it already failed. `antsy run` enforces
  them and stops the job the moment one trips, with a distinct exit code per mode.
- **No time estimates.** The agent never plans by the clock, never `sleep`s to wait, and never says
  "about ten minutes". Hooks deny long sleeps and unbounded poll loops. Waiting is event-driven.
- **A token-free watcher.** A background daemon checks watches (processes, logs, CI probes), notices
  dead supervisors, samples memory and reads OpenTelemetry. The agent hears about it only when something
  fails or finishes, through a plugin monitor, a background `antsy wait`, or a hook.
- **Nothing left hanging.** A job's whole process group dies with it. The Stop hook won't end a turn
  while failures are unread or detached work has nobody listening.
- **Traces on everything.** Jobs get `TRACEPARENT` and an OTLP endpoint. Error spans and ERROR logs
  from any instrumented program become failure events for the session that owns them.
- **Cheapest-first testing.** A cost ladder in the skill, a spend ledger with a daily cap that fails
  fast, and reminders on paid-API commands.
- **Attribution and allocation.** Runs and spend carry a purpose (`--for`), outcomes get noted, and
  `antsy account` shows where compute and money went, how much is unattributed, failure loops, and big
  purposes with nothing to show. For ML, evals and tests, the skill also asks for a map of the data
  (sources, slices, labels, splits) and for each result to be attributed to the slice or change that
  explains it. None of this is enforced; it keeps the questions in view.
- **Shared memory safety.** An admission check refuses heavy work below a free-memory floor instead of
  letting the machine swap. `antsy board` shows every agent's jobs with measured memory, and
  `CARGO_BUILD_JOBS` is sized to free memory. Antsy defers to an existing memory killer such as
  memguard, earlyoom or systemd-oomd, and never races it.

## Install

```sh
claude plugin marketplace add shinyobjectz/antsy
claude plugin install antsy@antsy
sh ~/.claude/plugins/marketplaces/antsy/scripts/install.sh   # binary: release download, else cargo build
```

Release binaries for macOS (arm64, x86_64) and Linux (x86_64, arm64, static musl) are on the
[releases page](https://github.com/shinyobjectz/antsy/releases); the install script picks the right one.

Or build it yourself: `cargo install --git https://github.com/shinyobjectz/antsy --locked`. The plugin's
`bin/antsy` shim finds the binary in `~/.antsy/bin`, `~/.cargo/bin`, or the plugin's `target/release`.

Check the install with `antsy doctor`. Nothing needs initializing. On an existing project the
session-start context points the agent at `antsy init`, which scans the project and drafts a
`.antsy.json`, and then at the setup decision tree in `skills/antsy/references/setup.md`.

## What the agent sees

At session start, the plugin injects the operating rules (`antsy creed` prints them). After that, the hooks
stay silent unless one of these happens:

| Hook | Says something when |
|---|---|
| PreToolUse (Bash) | a `sleep` over 5 s or an unbounded poll loop (denied); heavy work below the memory floor (denied); first heavy or paid command of the session (a one-line nudge); spend cap reached (denied) |
| PostToolUse, UserPromptSubmit | a failure or result arrived for this session since the last check |
| Stop | unread failures, or detached jobs and watches with nobody listening (blocks once) |
| SessionEnd | stops the session's watches and detached jobs, since nobody is left to hear them |
| Monitor `antsy` | streams this session's failures and results as they happen, even while idle |

## Usage

```sh
# A build under a contract, in the background; a compile error, hang or memory overrun stops it at once.
antsy run --name build --mem 12G -- cargo build --release

# Things you didn't start.
antsy watch pid 4242 --mem 8G
antsy watch log server.log --fail-on 'panic|level=error' --until 'listening on'
antsy watch check --every 30s --until '^success$' --fail-on '^(failure|cancelled)$' \
  -- gh run view 123456 --json conclusion -q .conclusion

antsy wait            # block until this session's next result; exit 1 on a failure
antsy events -f       # stream results and failures (what the plugin monitor runs)
antsy board           # memory, every agent's jobs and watches, today's spend
antsy log ID          # outcome and log tail
antsy traces --errors # spans received over OTLP (JSON or protobuf, plain or gzip) and job spans
antsy history build   # measured outcomes, durations, peaks; `--mem auto` caps from them
antsy stop ID         # or --mine
antsy spend 0.42 "3 judge calls" --for "eval: judge prompt v4"
antsy note ID "flake was a stale fixture" --useful
antsy account         # by purpose: runs, fail %, compute, peak, spend, useful/waste
```

Exit codes from `antsy run` are the child's own code, or one of these:

| Code | Meaning |
|---|---|
| 121 | stall |
| 122 | memory cap |
| 123 | a fail_on line |
| 124 | the max ceiling |
| 125 | refused at admission (memory or budget) |
| 126 | stopped on request |
| 128+N | killed by signal N |

## Configuration

Defaults are built into the binary. `~/.antsy/config.json` overrides them per user, and the nearest
`.antsy.json` overrides them per project; top-level keys replace the defaults.

```json
{
  "stall": "120s",
  "max": "2h",
  "fail_on": ["^\\s*error(\\[E\\d+\\])?:", "Traceback \\(most recent call last\\)"],
  "ignore": [],
  "free_floor_pct": 15,
  "mem_killer": "auto",
  "spend_cap_usd_day": 10,
  "otlp_port": 4318,
  "forward": "http://localhost:4319",
  "nudge": "once"
}
```

See [`skills/antsy/SKILL.md`](skills/antsy/SKILL.md) for the full philosophy, and `skills/antsy/references/`
for project setup, failure-mode recipes, telemetry, the cost ladder, data and allocation, and memory
and build guidance.

## How it works

```
 Claude Code session ──hooks (ms, file reads)──┐
   │ Bash: antsy run … ──supervises── job (own process group)
   │                         │ events, spans, job state
   ▼                         ▼
 monitor: antsy events -f ◀── ~/.antsy/events.jsonl ◀── antsy daemon
 background: antsy wait   ◀──┘   ▲  watches · dead supervisors · memory · OTLP/JSON on :4318
```

All state lives in plain files under `~/.antsy`: jobs, logs, events, traces, the spend ledger and run
history. The daemon starts on demand, runs once per user, and exits when idle. It is a single
dependency-light binary built from four pure-Rust crates (`libc`, `miniz_oxide`, `regex-lite`, `serde_json`).

Supported platforms are macOS and Linux.

## License

Apache License 2.0. See [LICENSE](LICENSE).
