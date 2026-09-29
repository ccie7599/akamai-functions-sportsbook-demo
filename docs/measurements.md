# Measurements

Everything below was measured on the deployed demo. It is what we observed, not
a platform guarantee. Rerun it yourself with the scripts in `scripts/`.

## Cold traffic

**Question.** Does a request that lands on a brand-new instance pay a start-up
penalty?

**Method.**

1. Deploy (`spin aka deploy`).
2. Immediately run `scripts/bench-cold.sh $URL 100`: 100 sequential requests to
   `/api/health`, each on a new TCP and TLS connection.
3. Record curl timings plus `x-fn-us` (time in the handler) and
   `x-instance-seq` (1 = brand-new instance) from each response.

**Result: 2026-09-29, client in Chicago (Linode us-ord), 100 requests right after a fresh deploy.**

| Metric | Value |
|---|---|
| Requests served by a brand-new instance | **100 of 100** |
| First request the new deployment served, time to first byte | **52.1 ms** |
| Time to first byte, p50 / p95, all 100 | 41.0 / 59.3 ms |
| Server share (TTFB minus TLS completion), p50 / p95 | 5.2 / 21.3 ms |
| Time inside the handler, p50 / p95 | 13 / 287 µs |

Raw data: [`bench-results/cold-after-deploy.csv`](../bench-results/cold-after-deploy.csv).

**Reading it.**

- **Every request started a new instance.** So "cold" isn't an edge case to
  engineer around: it is how every request runs, and it costs nothing visible.
- **The first request after the deploy sits inside the normal range**
  (52 ms against a 41–59 ms p50–p95). There is no warm-up curve.
- **Most of the client-side time is network setup.** The TLS handshake
  dominates each sample, because the benchmark deliberately opens a new
  connection per request. A browser reusing its connection sees single-digit
  milliseconds; the demo's chapter 7 runs the same test from your browser.

## Work inside the Function

Measured with `x-fn-us` and in-handler timers during the same session:

| Operation | Time inside the Function |
|---|---|
| Redirect lookup in the 100,000-entry table | 3–9 µs |
| Personalized board (build, sort, promos, boost) | ~0.3–0.6 ms |
| Scoring one slip (checks + model) | ~2–3 µs |
| Scoring 1,000 slips in one call | ~2.5–3 ms |

## Akamai Functions KV

A read from the built-in key-value store took about **22 ms** from the Function
(the first write took about 50 ms). That is why the redirect table is compiled
into the component, and KV is the tool for editable overrides rather than the
hot path.
