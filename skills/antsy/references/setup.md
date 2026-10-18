# Setting Antsy up on an existing project

Nothing has to be installed or initialized: the binary, the daemon and the hooks already work
everywhere with generic defaults. Setup makes them fit *this* project: its failure lines, its
heavy commands, what is already running, how it calls paid services, and what it traces.

Do it once per project, the first time you are about to start long-running work there (the
session-start context says when a project has no `.antsy.json`). Don't interrupt a small task
for it; a one-line fix needs no setup.

## Step 0: scan, don't explore

```sh
antsy init
```

It prints, without spending tokens on exploration: the project kinds (including monorepo
subfolders), processes already running out of the project, whether OpenTelemetry is a
dependency, paid SDKs in the manifests, CI, risky Rust profiles, and a draft `.antsy.json`.
Work through the tree below using that output; read files only where a branch asks you to.

## The tree

```
1. Is anything from this project already running?   (init: "running")
   ├─ yes, and someone (you, another agent, the user) is relying on it
   │    → antsy watch pid <pid> --mem <cap from its current RSS + headroom> --name <what it is>
   │      and, if it writes a log: antsy watch log <file> --fail-on '<its crash line>'
   ├─ yes, but it's orphaned (nobody owns it, it's from a dead session)
   │    → tell the user what it is and how much memory it holds; don't kill other people's work
   └─ no → go on

2. Are there long-running jobs in flight elsewhere?   (antsy board; CI runs; deploys)
   ├─ CI for the current branch is running
   │    → antsy watch check --every 30s --until '^success$' --fail-on '^(failure|cancelled)$' -- <status probe>
   └─ no → go on

3. What does failure look like here?   (init: draft fail_on)
   ├─ the draft covers the project's tools → keep it
   ├─ the project has its own error format (a logger, a test runner, a custom CLI)
   │    → run the narrowest failing-on-purpose command you can (one test, a typo build) under
   │      antsy run, read its output, and add its first-error line to fail_on
   └─ lines match fail_on but aren't failures (expected "error:" in test fixtures, noisy warnings)
        → add them to ignore, never loosen fail_on globally

4. What is heavy here?   (init: draft heavy)
   ├─ add the project's slow or memory-hungry commands (full builds, e2e suites, model loads,
   │  renders, docker builds) so the pre-Bash hook checks memory before they start
   └─ if there's history (~/.antsy/history.jsonl peak_mb for these names), note real peaks
      in the project's own docs or scripts as --mem caps

5. Does it spend money when tested?   (init: "paid SDKs"; grep for API base URLs if unsure)
   ├─ yes, and fakes or recorded fixtures exist → note how to run tests on them (the free rung)
   ├─ yes, and nothing free exists → propose the cheapest rung to add (references/cost.md);
   │  ask before adding a recording/replay dependency
   └─ no → go on

6. Can failures announce themselves?   (init: "telemetry")
   ├─ OpenTelemetry present → confirm the exporter reads OTEL_* env; error status on failures
   ├─ absent, and the project has services, workers or long jobs
   │    → propose instrumentation (references/telemetry.md); it's a code change, so do it as
   │      its own task with the user's OK, not silently during setup
   └─ absent, scripts only → antsy run's per-command spans are enough

7. Is the build memory-safe on a shared machine?   (init: "rust" line; other build configs)
   └─ fat LTO, codegen-units = 1, unbounded -j, many parallel test processes
        → propose lighter dev settings (references/memory-and-builds.md); release settings
          are the user's call

8. Write it down
   → antsy init --write (or save the edited draft as .antsy.json), and tell the user what
     you set up, what you're watching, and anything you proposed but didn't change.
     Commit .antsy.json with the project only if the user wants it shared.
```

## Opting out

A project whose `.antsy.json` is `{}` counts as set up and uses the global defaults; the
session-start pointer stops.

## Re-running

Run `antsy init` again after big changes, such as a new package in a monorepo, a new paid
service, or a new build system. It won't overwrite an existing `.antsy.json`; merge the new
draft by hand.
