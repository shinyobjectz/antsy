---
name: antsy
description: >-
  The fail-fast operating philosophy: define how work fails before starting it, never wait on a
  clock or estimate durations, leave nothing hanging, trace everything with OpenTelemetry, test
  cheapest-first, and treat machine memory as shared. Use it whenever you start anything that
  runs longer than a moment (builds, test suites, servers, renders, training, local models,
  deploys, CI), whenever you are tempted to sleep, poll, or say "this should take about N
  minutes", before any paid API or GPU call, before heavy local work on a shared machine, when
  adding logging or telemetry to code, and when deciding how to wait for something.
---

# Antsy

Antsy is impatient with failure, not with work. The goal is to learn that something has failed
at the first moment it is knowable, at the lowest cost, without a human or a model sitting and
watching. Waiting is an event, not a duration.

The plugin gives you a binary, `antsy`, and hooks that run on every session. The hooks are cheap
state-file reads; they speak only when something failed or a rule is about to be broken.

## The rules

1. **Decide how it fails before you start it.** Every job that runs longer than a moment gets a
   failure-mode contract before it begins: what liveness looks like and how long silence means a
   hang, how much memory it may hold, what it may cost, and which output lines mean it has already
   failed. Write the contract into the command, not your head:

   ```sh
   antsy run --name api-tests --stall 90s --mem 6G --fail-on 'FAILED|AssertionError' -- pytest -x tests/api
   ```

   If you can't say what failure looks like, you aren't ready to start. Read
   `references/failure-modes.md` for the contract checklist and recipes per kind of job.

2. **No clocks.** You can't estimate how long work takes, and plans built on estimates rot. So:
   - Never write "this will take ~10 minutes", "should be done in an hour", or plan work into time
     slots. Describe state instead: running, last output 4s ago, peak 2.1 GB, 312 of 900 tests.
   - Never `sleep` to wait for something. The pre-Bash hook denies sleeps over a few seconds and
     unbounded `while/until … sleep` loops.
   - Wait on events. Start the work with Bash `run_in_background` (you're notified when it exits),
     then keep doing useful work. For things you didn't start, add an `antsy watch` and a background
     `antsy wait`, or run the Monitor tool on `antsy events --follow`.
   - `--stall` is liveness (silence means hung), not a deadline. `--max` is a generous ceiling for
     things that are alive but never finish, not a prediction. Neither is a schedule.

3. **Leave nothing hanging.** Everything you start has an owner and something listening for it.
   Before you end a turn, every job is either finished, stopped (`antsy stop ID`), or has a
   waiter. The Stop hook blocks once when a detached job or watch has nobody listening, and when
   failures arrived that you haven't looked at. `antsy run` kills the job's whole process group
   when it ends, so nothing it started outlives it.

4. **Trace everything.** Programs run under `antsy run` get `TRACEPARENT`, `ANTSY_JOB`, and an
   OTLP/JSON endpoint pointing at the antsy daemon (127.0.0.1:4318) unless one is already set.
   When you write or touch code that does I/O, a network call, a queue, or a long computation,
   instrument it: spans around units of work, error status on failure, structured error logs.
   Error spans and ERROR logs become failure events for the session that owns them, without
   anyone reading the trace. See `references/telemetry.md`.

5. **Test cheapest first.** Before a call that costs money, climb the cost ladder: static checks,
   unit tests with fakes, recorded replays, a local or smaller model, one tiny real call, then the
   full paid run. Cache every paid response you might need again. Log real spend with
   `antsy spend <usd> <what>`; the daily cap fails fast, and paid-looking commands get a reminder.
   See `references/cost.md`.

6. **Memory is shared.** Other agents and the user's apps live on this machine. Before heavy local
   work (builds, local models, renders, big test suites), run `antsy board`: it shows free memory,
   every agent's running jobs with their measured memory, and any external memory killer. Give
   heavy jobs `--mem` (a cap) and `--need-mem` (admission): antsy refuses a job that would push
   free memory under the floor instead of letting the machine swap. Never add a second
   machine-wide killer where one exists; antsy defers to memguard, earlyoom, and systemd-oomd. Rust
   builds and local models have their own guidance in `references/memory-and-builds.md`.

7. **When woken, double-check.** A failure event is a lead, not a verdict. Read the log tail
   (`antsy log ID`), confirm with a second signal (exit code, test output, the trace), fix, and
   rerun the narrowest thing that proves the fix.

## Commands

| Command | Use |
|---|---|
| `antsy run [flags] -- <cmd>` | Run under a contract. Flags: `--name`, `--stall 2m`, `--max 2h`, `--mem 8G`, `--need-mem 4G`, `--fail-on RE` (repeatable), `--no-default-fail-on`, `--grace 3s`, `--progress-file F`, `--cost-usd 0.40`, `-q`. |
| `antsy watch pid <pid> [--mem 8G] [--stall 5m --progress-file F]` | Watch a process you didn't start. |
| `antsy watch log <file> [--fail-on RE] [--until RE] [--stall 2m]` | Fail on a log line, finish on another. |
| `antsy watch check --every 30s [--until-ok] [--until RE] [--fail-on RE] -- <probe>` | Cheap polling done by the daemon (CI status, a health URL, a file appearing). |
| `antsy wait [ID…]` | Block until listed IDs end (exit 1 on a failure), or until this session's next result. Run with `run_in_background`. Returns at once if nothing is running. |
| `antsy events [-f] [--all]` | Recent results and failures; `-f` streams them for the Monitor tool. |
| `antsy board [--mine]` | Memory, every agent's jobs and watches, today's spend. |
| `antsy log ID` / `antsy stop ID…` / `antsy stop --mine` | Inspect or stop. |
| `antsy spend [USD WHAT]` | Spend ledger with a daily cap (exit 3 over it). |

Exit codes from `antsy run`: the child's own code, or 121 stall, 122 memory cap, 123 fail_on line,
124 max ceiling, 125 refused at admission (memory or budget), 126 stopped on request, 128+N signal.

## Recipes

**A build or test suite.** Run it in the background with a contract, then keep working:

```sh
antsy run --name build --mem 12G -- cargo build --release     # Bash with run_in_background: true
```

You're notified when it exits. A compile error stops it within the grace period, a hang stops at
`--stall`, and memory overruns stop at `--mem`.

**A dev server you need to stay up.** Start it under `antsy run` in the background, then confirm
readiness with a bounded probe instead of sleeping:
`antsy run --max 30s --name ready -- sh -c 'until curl -sf localhost:3000/health; do sleep 1; done'`.

**CI or a deploy you can't see.** Let the daemon poll for free, and wake on the result:

```sh
antsy watch check --name ci --every 30s --until '^success$' --fail-on '^(failure|cancelled)$' \
  -- gh run view 123456 --json conclusion -q .conclusion
antsy wait        # Bash with run_in_background: true
```

**Something already running.** `antsy watch pid <pid> --mem 8G`, or `antsy watch log app.log`
for its log. Then a background `antsy wait`.

**Before ending a turn.** Run `antsy board --mine`. Stop what you no longer need; make sure a waiter
or the plugin monitor covers the rest.

## How you hear about failures without spending tokens

The `antsy daemon` (started on demand, one per user, exits when idle) evaluates watches, notices
supervisors that vanished (and kills their process groups), samples memory, and reads OTLP
traffic. It writes events to `~/.antsy/events.jsonl`. You hear about them through four channels,
none of which costs tokens while nothing is happening:
- the plugin monitor (`antsy events --follow`), which streams this session's failures as they happen;
- `antsy wait` in a background Bash, which exits on the next result and wakes you;
- the PostToolUse and UserPromptSubmit hooks, which add any undelivered failures to your context;
- the Stop hook, which won't let you end a turn while unseen failures or unwatched work remain.

For waiting on state antsy can't see at all, `/loop` with a long ScheduleWakeup interval is the
fallback heartbeat; pace it to how fast the state actually changes, and prefer an `antsy watch
check` probe that turns it into an event.

## Configuration

Defaults live in the binary. Override per user in `~/.antsy/config.json` and per project in the
nearest `.antsy.json` (top-level keys replace defaults): `stall`, `max`, `fail_on`, `ignore`, `grace`,
`free_floor_pct`, `mem_killer` (`auto` | `antsy` | `off`), `external_killers`, `sleep_max_s`, `heavy`,
`paid`, `spend_cap_usd_day`, `otlp_port`, `forward` (an OTLP/HTTP collector to copy traces to),
`idle_exit`, and `nudge` (`once` | `always` | `off`). A project should commit its `.antsy.json` with
its own failure lines and heavy commands.
