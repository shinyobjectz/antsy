# Testing cheaply

Failing fast is also the cheapest way to fail. Every paid call should be the last rung of a
ladder whose lower rungs have already caught everything they can.

## The ladder

1. **Static.** Types, lints, schema validation of the request you are about to send. Free.
2. **Fakes.** Unit tests with the client mocked at the boundary. Free.
3. **Recorded replay.** Record one real response, commit it as a fixture, replay it in tests
   (VCR-style cassettes, `msw`, `responses`, `wiremock`, a JSON file). Free after the first call.
4. **Local or smaller.** A local model, the provider's smallest model, a sandbox or test-mode API
   (Stripe test keys, provider sandboxes), the emulator.
5. **One tiny real call.** The smallest input that exercises the path: 1 item, 1 frame, 10 tokens,
   `max_tokens` capped, one region.
6. **The full run**, only after 1–5 pass, with a cost line in the contract.

Skipping a rung needs a reason. "It's faster to just run it" usually means paying to find a bug a
free rung would have caught.

## Habits

- **Cache paid responses** by request hash, so a rerun after an unrelated fix costs nothing.
- **Cap everything a request can grow by**: `max_tokens`, `n`, image count, video seconds, batch
  size, retries. An uncapped retry loop against a paid API is the classic expensive failure.
- **Dry-run flags first** (`--dry-run`, `plan`, `--check`, `--what-if`).
- **Fail before paying.** Validate inputs, credentials, and quotas before the expensive call, not
  after it fails halfway.
- **Batch and reuse.** One call with 20 items beats 20 calls; reuse uploaded files and prompts
  (prompt caching where the provider offers it).
- **Put cost on the trace**: model, tokens, unit price, cache hits as span attributes.

## The ledger

```sh
antsy spend 0.42 "openrouter: 3 judge calls on the eval smoke set"
antsy spend          # today's total and the cap
antsy run --cost-usd 2.50 --name full-eval -- ./eval.sh    # refused if it would cross the cap
```

The daily cap (`spend_cap_usd_day`, default $10) fails fast: `antsy spend` exits 3 over it,
`antsy run --cost-usd` refuses to start, and the pre-Bash hook denies commands that match `paid`
patterns. Raising the cap is the user's decision, not yours.
