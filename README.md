# Edge Book: a sportsbook request path on Akamai Functions

A working reference for running application logic on the request path with
[Akamai Functions](https://techdocs.akamai.com/cloud-computing/docs/akamai-functions),
using a sportsbook as the example. One WebAssembly component, written in Rust
with [Spin](https://spinframework.dev), serves the UI and every API. There are
no servers, containers or scaling policies behind it.

Each part of the demo is a common edge use case, told as a step in a bettor's
journey:

| Chapter | Use case | What the Function does on each request |
|---|---|---|
| 1. The bettor arrives | Mass redirects | Resolves one of 100,000 campaign codes compiled into the component, legacy URL rules, and jurisdiction routing |
| 2. The board they see | Hyper-personalization | Builds the board for this customer: favorite team first, promotions by segment and state, and a profit boost sized to live price movement |
| 3. Who else is looking | Bot misdirect | Serves automated clients plausible but consistently wrong prices, instead of blocking them |
| 4. The price they lock | Token management | Signs each quote (HMAC-SHA256, 30 s expiry) and verifies it before anything else runs |
| 5. The bet they place | AI inference | Checks the slip and scores it with a small open-weight model; 1,000 slips score in ~3 ms |
| 6. It was all one Function | Full-stack application | Everything above is one deployable component, and it serves its own source |
| 7. Zero penalty for cold traffic | Platform | Every request runs in a brand-new sandbox, with no start-up penalty (see [measurements](docs/measurements.md)) |
| 8. Streams and sockets | Architecture | Where Functions hand off to a stream tier for WebSocket and SSE |

All data is synthetic. Team names are real clubs for readability; prices,
promotions, jurisdiction rules and the risk model are invented for the demo.

## Quick start

Prerequisites: [Rust](https://rustup.rs) with the `wasm32-wasip1` target, and the
[Spin CLI](https://spinframework.dev/v3/install) (3.x).

```bash
git clone https://github.com/ccie7599/akamai-functions-sportsbook-demo
cd akamai-functions-sportsbook-demo
rustup target add wasm32-wasip1

make run            # build and serve locally on http://127.0.0.1:3777
```

Open the URL and use the sidebar (or the arrow keys) to walk the chapters.

### Deploy to Akamai Functions

You need an Akamai Functions account and the `aka` Spin plugin:

```bash
spin plugins install aka
spin aka login
make deploy         # generates a signing key into .env on first run
```

`make deploy` prints the app URL (`https://<id>.fwf.app`). Then:

```bash
make smoke URL=https://<id>.fwf.app          # every route, with timings
make bench URL=https://<id>.fwf.app          # cold-traffic benchmark
```

## How it is built

```
.
├── spin.toml                     # app manifest: route, variables, allowed outbound hosts
├── edge-book/
│   ├── build.rs                  # generates the 100,000-entry redirect table at build time
│   ├── model/risk-stand-in-v1.json   # stand-in model weights and thresholds
│   ├── static/index.html         # the demo UI (embedded in the component)
│   └── src/
│       ├── lib.rs                # router, timing headers, source viewer
│       ├── book.rs               # synthetic book: events, markets, prices
│       ├── redirects.rs          # chapter 1
│       ├── personalize.rs        # chapter 2
│       ├── scraper.rs            # chapter 3
│       ├── token.rs              # chapter 4
│       └── risk.rs               # chapter 5
├── scripts/
│   ├── bench-cold.sh             # cold-traffic benchmark
│   └── smoke.sh                  # call every route
└── docs/
    ├── architecture.md           # request flow, component design, data sources
    ├── use-cases.md              # each chapter: problem, implementation, curl examples
    └── measurements.md           # what was measured, how, and the results
```

Start with [docs/architecture.md](docs/architecture.md), then
[docs/use-cases.md](docs/use-cases.md).

### API

| Method | Route | Purpose |
|---|---|---|
| GET | `/` | Demo UI |
| GET | `/go/{code}?state=XX` | Campaign redirect (302) |
| GET | `/api/redirect?path=&state=` | Explain how a path would redirect |
| GET | `/api/board?segment=&state=&fav=` | Personalized board (or poisoned, for automated clients) |
| POST | `/api/quote` | `{"selection_id"}` → signed quote token |
| POST | `/api/precheck` | `{"quote_token","stake","segment","bettor"}` → checks and risk score |
| GET | `/api/simulate?n=1000` | Score n synthetic slips, return the decision mix and time per slip |
| GET | `/api/meta` | Routes, build info, redirect table size |
| GET | `/src/{file}` | Source of the running component |
| GET | `/api/health` | Current price version |

Every response carries two headers that make the platform's behavior visible:

- `x-fn-us`: microseconds spent inside the handler.
- `x-instance-seq`: requests served by this instance. It is a counter in the
  component's memory that starts at zero in every new instance, so `1` means
  the request ran in a brand-new sandbox.

### Configuration

| Variable | Default | Purpose |
|---|---|---|
| `quote_signing_key` | `dev-only-change-me` | HMAC key for quote tokens. Always set a real one on deploy (`make deploy` does). |
| `feed_url` | empty | Empty: synthetic book computed from the clock. Set to any HTTPS JSON endpoint in the same schema to use real data; add its host to `allowed_outbound_hosts` in `spin.toml`. |

## Adapting it

- **Use your own data.** Point `feed_url` at your price feed, or replace
  `load_book` in `lib.rs` with a call to Akamai Functions KV, a database API or
  object storage. Everything downstream works on the `Book` type.
- **Use your own model.** Replace `model/risk-stand-in-v1.json` with your
  weights, or swap the scoring in `risk.rs` for your model's runtime. The
  features are computed in `evaluate()`.
- **Use your own bot verdict.** In production, read the verdict your bot
  management layer adds to the request instead of the heuristics in
  `scraper.rs`.
- **Put it behind a property.** Route a path on an Akamai property to the
  Function's `fwf.app` hostname to serve it on your own domain.

## Limitations

- Prices, promotions, jurisdiction rules and the risk model are synthetic and
  illustrative. Nothing here reflects any operator's actual rules or models.
- Scraper detection uses simple signals (automation user-agents, missing
  `accept-language`) so it can be reproduced with curl.
- The UI refreshes by polling once a second. Streaming (WebSocket/SSE) needs a
  stream tier; see chapter 8 and [docs/architecture.md](docs/architecture.md).

## License

Apache 2.0. See [LICENSE](LICENSE).
