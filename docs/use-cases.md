# Use cases

Each chapter of the demo follows the same shape: the business problem, how the
Function solves it, and how to see it yourself. Set `URL` to your deployment
(or `http://127.0.0.1:3777` when running `make run`).

```bash
URL=https://<id>.fwf.app
```

---

## 1. Mass redirects: the bettor arrives

**Problem.** Customers arrive through podcast codes, affiliate links, QR codes,
old bookmarks, and a front door that must send them to the right state's site.
On a big weekend that is millions of requests whose only job is a lookup.

**Implementation.** `build.rs` generates 100,000 campaign codes at build time,
stored as sorted fixed-width records, 10 bytes each. `redirects.rs` resolves a
path in three layers:

1. A campaign code is found by binary search. Welcome offers get a
   state-specific landing page in the demo's (fictional) restricted states.
2. Legacy URL rules rewrite old paths to new ones (301).
3. The front door `/` redirects to the customer's jurisdiction.

**Try it.**

```bash
CODE=$(curl -s $URL/api/meta | jq -r '.redirect_samples[1]')
curl -sI "$URL/go/$CODE?state=NJ" | grep -i location
curl -s  "$URL/api/redirect?path=/go/$CODE&state=NY&explain=1" | jq
curl -s  "$URL/api/redirect?path=/sportsbook/leagues/nfl" | jq
```

**Value.** Lookups take single-digit microseconds (`lookup_ns` in the response)
and never touch an origin.

---

## 2. Hyper-personalization: the board they see

**Problem.** A new customer, a VIP and a lapsed customer should each see a
different page. That means favorite team first, offers filtered by segment and
jurisdiction, and boosts that don't become a liability when the price is moving.

**Implementation.** `personalize.rs` runs on every `/api/board` request:

- Events are sorted with the favorite team first.
- Promotions are filtered by segment and jurisdiction.
- A profit boost is applied to one featured price, then trimmed in proportion
  to how far that price moved in the last 10 seconds, and paused while the
  market is suspended.

Nothing is cached per user.

**Try it.**

```bash
curl -s -A Mozilla/5.0 -H 'accept-language: en' "$URL/api/board?segment=new&state=NJ&fav=Lions" \
  | jq '.promos, [.events[].markets[].selections[] | select(.boost) | .boost]'
curl -s -A Mozilla/5.0 -H 'accept-language: en' "$URL/api/board?segment=new&state=NY&fav=Lions" | jq '.promos'
```

**Value.** A full personalized board is built in well under a millisecond inside
the Function (`x-fn-us`).

---

## 3. Bot misdirect: who else is looking

**Problem.** Odds screens and arbitrage bots scrape prices constantly. Blocking
them tells them to rotate. Data that looks right but is quietly wrong makes
their output worthless and gives them nothing to react to.

**Implementation.** `scraper.rs` classifies the request. In the demo it uses
simple signals: automation user-agents and a missing `accept-language`. In
production you'd read your bot management verdict instead. For an automated
client the Function:

- shifts every price by a small, stable amount,
- nudges some lines by half a point, and
- shows suspended markets as open.

The noise is keyed to the client's fingerprint and a 30-second window, so it is
consistent across requests and can't be averaged away. Nothing in the response
reveals the verdict.

**Try it.**

```bash
curl -s -A 'python-requests/2.31' $URL/api/board | jq '.events[0].markets[0].selections[] | {name, price}'
curl -s -A Mozilla/5.0 -H 'accept-language: en' $URL/api/board | jq '.events[0].markets[0].selections[] | {name, price}'
```

---

## 4. Token management: the price they lock

**Problem.** A bet slip carries a price. If the client can edit it, or replay a
quote after the line moved, the book pays. Quotes must be issued and verified
where the request arrives.

**Implementation.** `token.rs` signs `{selection, price, price version, expiry}`
with HMAC-SHA256 (`payload.signature`, base64url). `risk.rs` verifies the token
before it does anything else and trusts only the signed fields. The key is the
secret Spin variable `quote_signing_key`.

**Try it.**

```bash
TOKEN=$(curl -s -X POST $URL/api/quote -d '{"selection_id":"nfl-kc-buf-ml-away"}' | jq -r .token)
curl -s -X POST $URL/api/precheck -d "{\"quote_token\":\"$TOKEN\",\"stake\":40,\"bettor\":{}}" | jq '.decision, .checks'
# Tamper: flip one character in the payload and submit again -> "quote signature invalid"
```

---

## 5. AI inference: the bet they place

**Problem.** Live bets often carry a fixed acceptance delay to protect against
bettors who see the play before the price moves. Every customer pays it.
Scoring each slip as it arrives lets harmless bets through immediately and
saves the delay, and the trader's attention, for the ones that need it.

**Implementation.** `risk.rs` runs deterministic checks first: signature,
market open, price still valid, stake within limits. It then computes five
features:

- stake relative to the bettor's usual stake
- sharpness
- quote age
- in-play
- betting velocity

The features are scored with a logistic model loaded from
`model/risk-stand-in-v1.json`. Decisions:

- `accept_fast`: no delay
- `accept_standard`: standard delay
- `trader_review`
- `reject`

Acceptance itself stays on the regulated platform.

**The model is a stand-in with hand-set weights.** Replace it with your own
tuned open-weight model: new weights in the same format, or your model's
runtime in `evaluate()`.

**Try it.**

```bash
curl -s "$URL/api/simulate?n=1000" | jq '{decisions, by_profile, us_per_slip}'
```

---

## 6. Full-stack application: it was all one Function

The UI, nine routes, the redirect table, token signing and the model are one
component, built with `spin build` and deployed with `spin aka deploy`. The
component serves its own source at `/src/{file}`.

```bash
curl -s $URL/api/meta | jq '.routes, .sources'
curl -s $URL/src/token.rs | head -20
scripts/smoke.sh $URL
```

---

## 7. Zero penalty for cold traffic

Every request runs in a brand-new sandbox. `x-instance-seq` is `1` on every
response, and the first request after a deploy performs like the rest. See
[measurements.md](measurements.md) for the method and results, and reproduce
with:

```bash
make deploy && make bench URL=$URL
```

---

## 8. Streams and sockets

Functions are request and response. For WebSocket or SSE, issue a signed stream
ticket from a Function (same pattern as chapter 4) and hold the connections in
a stream tier on Akamai cloud. See
[architecture.md](architecture.md#streams-and-sockets).
