# Telemetry

Traces exist so failures can announce themselves. A span with error status reaches the owning
session as a failure event; a healthy span is data you can query later instead of re-running.

## What antsy gives a job

`antsy run` sets these for the child, without overriding values that are already set:

- `TRACEPARENT=00-<trace>-<span>-01`: the job's own span is the parent, so the program's spans join
  the job's trace.
- `OTEL_EXPORTER_OTLP_ENDPOINT=http://127.0.0.1:4318` and, if unset, `OTEL_EXPORTER_OTLP_PROTOCOL=http/protobuf`.
- `OTEL_RESOURCE_ATTRIBUTES` gains `antsy.job=<id>,antsy.session=<session>`, which routes failures
  back to the right session.
- `ANTSY_JOB`, `ANTSY_SESSION`.

The daemon reads OTLP/HTTP traces and logs as JSON or protobuf, plain or gzip, so SDK defaults
work unchanged. gRPC (port 4317) isn't served; use the HTTP exporter. Metrics are accepted and
kept but not decoded. Every batch is kept in `~/.antsy/traces/<day>.jsonl` as OTLP/JSON. Set `forward` in config to copy
batches to a real collector (Jaeger, Tempo, Honeycomb, an OTel Collector).

## Reading traces back

```sh
antsy traces --job build-3f2a1     # the job's span and every span its program sent
antsy traces --errors --days 2     # every failed span
antsy traces --service api --slow 500
antsy traces --json -n 200         # for scripts
```

## What counts as a failure

- A span with `status.code` 2 (ERROR).
- A log record with `severityNumber` 17 or higher (ERROR, FATAL).

Spans without `antsy.session` reach every session as global failures, so set the attribute (or
run the program under `antsy run`) when several agents share a machine.

## Instrumenting code you write

- One span per unit of work a person would name: a request, a job, a migration step, a model call.
  Name spans by operation (`POST /checkout`, `render frame batch`), not by function.
- Set error status with a short message at the point of failure, and record the exception.
- Put cost-relevant attributes on spans: model, tokens in/out, bytes, rows, cache hit.
- Log at ERROR only for real failures; anything at ERROR wakes an agent.
- Prefer the language's official OpenTelemetry SDK with the OTLP HTTP exporter, configured only
  from the standard `OTEL_*` environment variables, so antsy, CI, and production can each point it
  somewhere else without code changes.

## Shell and one-off scripts

A script doesn't need an SDK: `antsy run` already makes a span for the whole command with its exit
code, mode, peak memory, and log path. Split a long script into several `antsy run` steps when
you want a span per step.
