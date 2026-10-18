# Memory, builds, and local models

A machine that swaps fails slowly for everyone on it, which is the worst kind of failure. Antsy's
memory rules turn "slow for everyone" into "refused fast for one job".

## Shared-machine protocol

1. Run `antsy board` before heavy work. It shows free memory, pressure, and every agent's running
   jobs with their **measured** memory, not anyone's guess.
2. Give heavy jobs `--need-mem` (what must be free to start) and `--mem` (the cap). Antsy refuses
   to start a job that would leave less than `free_floor_pct` free.
3. A refusal is information. Don't retry in a loop. Either wait on an event (`antsy wait ID` on the
   job holding the memory), shrink the job, or ask.
4. Running under `antsy run` is the announcement: other agents see the job, its live memory, and
   its peak on their board.
5. One machine-wide memory killer at most. If memguard, earlyoom, systemd-oomd, or nohang is
   running, antsy never kills at the memory edge (`mem_killer: auto`). Without one, antsy stops only
   its own largest job, and only at critical pressure.

## Rust builds

`antsy run` sets `CARGO_BUILD_JOBS` from free memory (about 1.5 GB per rustc job) when it isn't
already set. Beyond that:

- **Profiles.** Keep dev builds light: `debug = "line-tables-only"` (or `0`) in `[profile.dev]`,
  `opt-level = 1` for `[profile.dev.package."*"]` when tests are slow. In release use `lto = "thin"`
  rather than fat LTO, and `codegen-units` above 1 unless you measured a win; fat LTO with one
  codegen unit is the most memory-hungry link there is.
- **Check before build.** `cargo check` (and `clippy`) for type errors; build only to run.
- **Narrow scope.** `-p crate`, `--bin`, `--test name`, `--lib`. In a workspace, build the crate you
  changed.
- **Linkers.** Linking is often the memory peak. Use `lld` or `mold` on Linux, and on macOS the
  default `ld-prime`. Split giant binaries rather than raising caps.
- **Dependencies.** Each heavy dependency (and each feature flag on one) is paid on every cold
  build. Prefer small crates and default-features = false.
- **Shared target dirs** (`CARGO_TARGET_DIR`) save disk and rebuilds across worktrees, but two
  builds into the same target dir serialize on cargo's lock; treat that wait as an event, not as
  a hang.
- **sccache** helps repeated cold builds when there's disk to spare for its cache.
- Record peaks: every named job's `peak_mb` is kept. `antsy history NAME` shows them, and
  `--mem auto` caps the next run at 30% over the highest recent peak.

Other compiled languages follow the same rules: `swift build -j`, `make -j` from free memory,
`xcodebuild -jobs`, Gradle `--max-workers`, `go build -p`.

## Local models

- **Load one at a time** per machine unless the board says there's room for two. Model loads are
  the fastest way to push a machine into swap.
- **Size before loading**: weights plus KV cache (context length × layers × heads) plus runtime.
  Pick the smallest quantization that passes the test, and cap context length.
- **Cap it**: `antsy run --need-mem 10G --mem 14G -- llama-server …`. A model server under a cap
  fails as `mem` instead of freezing the desktop.
- **Unload when done.** A model server left running is a hanging job; stop it or give it a waiter.
- **Accelerators.** GPU, MPS, or Neural Engine compiles can spike memory far beyond the weights.
  Do the first compile of a new model when the board is quiet, and record its peak.
- **Prefer the smallest model that proves the point** (see cost.md). The cheapest local test is
  the one that doesn't load a model at all: test the plumbing with a stub that returns fixed
  outputs.
