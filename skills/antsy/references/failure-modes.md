# Failure-mode contracts

A contract answers five questions before the job starts. If you can't answer one, find out with
the cheapest possible probe first (a dry run, `--help`, one input) rather than starting the full
job and seeing what happens.

| Question | Flag | How to pick it |
|---|---|---|
| What does progress look like? | `--stall`, `--progress-file` | Output lines by default. For silent jobs (linkers, renders, downloads), point `--progress-file` at a file that grows or is touched as work advances. Pick a silence that would mean "hung", not "slow". |
| Which lines mean it already failed? | `--fail-on RE`, `.antsy.json` `fail_on` | The first error usually decides the run. Defaults cover compiler errors, panics, tracebacks, OOM, `npm ERR!`, and `** BUILD FAILED **`. Add the project's own: `FAILED`, `AssertionError`, `level=error`, `HTTP 5..`. |
| How much memory may it hold? | `--mem` (cap), `--need-mem` (admission) | `--mem auto` uses the measured peaks of earlier runs with the same `--name` (`antsy history NAME`). With no history, start small and let the cap tell you. |
| What may it cost? | `--cost-usd`, `antsy spend` | See cost.md. A paid job without a cost line is a contract with a hole in it. |
| When is it pointless to continue? | `--max` | A generous ceiling for things that are alive but never finish (an infinite retry loop that prints). It isn't an estimate, and nothing gets planned around it. |

## Recipes by kind of job

**Compilers.** Defaults catch `error:` and `error[E…]:`. `--grace 3s` (the default) lets the compiler
finish printing that batch of errors before the stop, so one rebuild can fix several.

**Test suites.** Fail on the first failing test when the suite is slow: `pytest -x`, `jest --bail`,
or `cargo test` with `--fail-on '^test .* FAILED$'`. Run the narrowest set that can
prove the change before the full suite.

**Servers.** Start them under `antsy run` in the background with `--fail-on` for their crash lines;
probe readiness with a bounded `until` loop under `antsy run --max 30s`. Don't sleep and hope.

**Renders, encodes, training.** These are often quiet. Use `--progress-file` on the output file or
a checkpoint, and a `--mem` cap from the last run's peak.

**Downloads.** `--progress-file` on the partial file. A download that stops growing is a stall.

**Things already running.** `antsy watch pid` (exit plus a memory alarm) and `antsy watch log`
(fail and finish lines plus a stall).

**Remote things (CI, deploys, queues).** `antsy watch check` with a probe that prints the state;
`--until` on the success state and `--fail-on` on the failure states.

## Reading an outcome

| Mode | Exit | Meaning | First move |
|---|---|---|---|
| `fail_on` | 123 | A failure line appeared | `antsy log ID`: the matched line is in `why` |
| `stall` | 121 | No output or progress for `--stall` | Look at what it was doing last: deadlock, network wait, prompt for input (stdin is closed, so prompts fail fast) |
| `mem` | 122 | Process group over `--mem` | Smaller input, fewer workers, streaming instead of loading, or a measured larger cap |
| `max` | 124 | Hit the ceiling while alive | Usually a retry loop or a job that should have been split |
| `refused` | 125 | Not started: memory floor or spend cap | `antsy board`: wait on someone's job ending (an event), or shrink the job |
| `vanished` | n/a | Supervisor killed from outside (OOM killer, memguard, closed terminal) | Check the external killer's log; the job's process group was stopped |
| `exit` | child's | It finished with a non-zero code | `why` holds the first failure-looking line if there was one |
