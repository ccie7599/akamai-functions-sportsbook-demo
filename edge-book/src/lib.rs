//! Edge Book: a sportsbook request path running as one WebAssembly component on
//! Akamai Functions.
//!
//! Every route is handled end to end in this component: the UI, redirects,
//! personalization, scraper handling, signed quotes and pre-trade risk scoring.
//!
//! Two response headers make the platform's behavior visible:
//! - `x-fn-us`: microseconds spent inside the handler.
//! - `x-instance-seq`: how many requests this instance has served. A static
//!   counter starts at zero in every new instance, so `1` on every response means
//!   every request ran in a freshly created sandbox.

mod book;
mod personalize;
mod redirects;
mod risk;
mod scraper;
mod token;

use serde_json::{json, Value};
use spin_sdk::http::{IntoResponse, Method, Request, Response};
use spin_sdk::http_component;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

const INDEX: &str = include_str!("../static/index.html");

/// Starts at 0 in every new instance of this component.
static INSTANCE_SEQ: AtomicU64 = AtomicU64::new(0);

/// Source files the demo UI can show, served straight from this component.
const SOURCES: &[(&str, &str)] = &[
    ("lib.rs", include_str!("lib.rs")),
    ("book.rs", include_str!("book.rs")),
    ("personalize.rs", include_str!("personalize.rs")),
    ("scraper.rs", include_str!("scraper.rs")),
    ("token.rs", include_str!("token.rs")),
    ("risk.rs", include_str!("risk.rs")),
    ("redirects.rs", include_str!("redirects.rs")),
    ("risk-stand-in-v1.json", include_str!("../model/risk-stand-in-v1.json")),
    ("spin.toml", include_str!("../../spin.toml")),
];

fn now_ms() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
}

fn query(req: &Request, key: &str) -> Option<String> {
    req.query().split('&').find_map(|kv| {
        let (k, v) = kv.split_once('=')?;
        (k == key && !v.is_empty()).then(|| v.replace("%20", " ").replace('+', " ").replace("%2F", "/"))
    })
}

fn header(req: &Request, name: &str) -> Option<String> {
    req.header(name).and_then(|h| h.as_str()).map(str::to_string)
}

struct Out {
    status: u16,
    ctype: &'static str,
    body: String,
    extra: Vec<(&'static str, String)>,
}

fn json_out(status: u16, v: &Value) -> Out {
    Out { status, ctype: "application/json", body: v.to_string(), extra: vec![] }
}

/// The book at `ms`: from `feed_url` when configured, else synthetic.
async fn load_book(ms: u64) -> book::Book {
    let url = spin_sdk::variables::get("feed_url").unwrap_or_default();
    if !url.is_empty() {
        let req = Request::get(url.as_str()).build();
        if let Ok(resp) = spin_sdk::http::send::<_, Response>(req).await {
            if *resp.status() == 200 {
                if let Ok(mut b) = serde_json::from_slice::<book::Book>(resp.body()) {
                    b.source = format!("feed: {}", url);
                    return b;
                }
            }
        }
    }
    book::synthetic(ms)
}

async fn board(req: &Request) -> Out {
    let ms = now_ms();
    let mut b = load_book(ms).await;
    let prev = book::synthetic(ms.saturating_sub(10_000));

    let ua = header(req, "user-agent").unwrap_or_default();
    let ip = header(req, "true-client-ip").or_else(|| header(req, "x-forwarded-for")).unwrap_or_default();
    let forced = query(req, "view").as_deref() == Some("scraper");
    let verdict = scraper::detect(&ua, header(req, "accept-language").as_deref(), &ip, forced);

    // Scrapers get poisoned prices and no personalization. The demo flag
    // `debug=1` reveals the verdict; a real scraper never sees it.
    if let Some(v) = verdict {
        scraper::poison(&mut b, v.fingerprint);
        let mut body = json!({ "version": b.version, "source": b.source, "events": b.events, "promos": [] });
        if query(req, "debug").as_deref() == Some("1") {
            body["scraper"] = json!({ "detected": true, "signals": v.signals });
        }
        return json_out(200, &body);
    }

    let ctx = personalize::Ctx {
        segment: query(req, "segment").unwrap_or_else(|| "casual".into()),
        state: query(req, "state").unwrap_or_else(|| "NJ".into()),
        fav: query(req, "fav"),
    };
    let promos = personalize::apply(&mut b, &prev, &ctx);
    json_out(200, &json!({
        "version": b.version,
        "source": b.source,
        "context": { "segment": ctx.segment, "state": ctx.state, "fav": ctx.fav },
        "events": b.events,
        "promos": promos,
    }))
}

/// Issue a signed quote for the selection's current price (boosted if boosted).
async fn quote(req: &Request) -> Out {
    let body: Value = serde_json::from_slice(req.body()).unwrap_or(Value::Null);
    let Some(sel) = body.get("selection_id").and_then(Value::as_str) else {
        return json_out(400, &json!({ "error": "selection_id required" }));
    };
    let ms = now_ms();
    let b = load_book(ms).await;
    let Some((_, m, s)) = book::find(&b, sel) else {
        return json_out(404, &json!({ "error": "unknown selection" }));
    };
    if m.suspended {
        return json_out(409, &json!({ "error": "market suspended" }));
    }
    let q = token::Quote { sel: sel.to_string(), price: s.price, ver: b.version, exp: ms / 1000 + token::TTL_S };
    let tok = token::sign(&q, &token::key());
    json_out(200, &json!({ "quote": q, "token": tok, "ttl_s": token::TTL_S }))
}

async fn precheck(req: &Request) -> Out {
    let Ok(pr) = serde_json::from_slice::<risk::PrecheckReq>(req.body()) else {
        return json_out(400, &json!({ "error": "invalid precheck body" }));
    };
    let ms = now_ms();
    let b = load_book(ms).await;
    json_out(200, &risk::precheck(&b, &pr, ms / 1000))
}

async fn simulate(req: &Request) -> Out {
    let n = query(req, "n").and_then(|v| v.parse().ok()).unwrap_or(1000usize).min(20_000);
    let ms = now_ms();
    let b = load_book(ms).await;
    json_out(200, &risk::simulate(&b, n, ms))
}

fn redirect(req: &Request, path: &str) -> Out {
    let state = query(req, "state").unwrap_or_else(|| "NJ".into());
    let t = Instant::now();
    let r = redirects::resolve(path, &state);
    let lookup_ns = t.elapsed().as_nanos();
    if path.starts_with("/go/") && query(req, "explain").is_none() {
        return Out { status: r.status, ctype: "text/plain", body: format!("Redirecting to {}\n", r.location), extra: vec![("location", r.location.clone())] };
    }
    let mut v = serde_json::to_value(&r).unwrap_or(Value::Null);
    v["lookup_ns"] = json!(lookup_ns);
    json_out(200, &v)
}

fn meta() -> Out {
    json_out(200, &json!({
        "component": "edge-book",
        "build": { "time": option_env!("BUILD_TIME").unwrap_or("dev"), "git": option_env!("GIT_SHA").unwrap_or("dev") },
        "routes": [
            "GET  /                      demo UI",
            "GET  /go/{code}             campaign redirect (302)",
            "GET  /api/redirect?path=    explain a redirect",
            "GET  /api/board             personalized or poisoned board",
            "POST /api/quote             signed quote token",
            "POST /api/precheck          verify quote, checks, risk score",
            "GET  /api/simulate?n=       score n synthetic slips",
            "GET  /api/meta              this",
            "GET  /src/{file}            source of this component",
        ],
        "redirect_codes": redirects::table_size(),
        "redirect_samples": redirects::samples(),
        "sources": SOURCES.iter().map(|(n, _)| *n).collect::<Vec<_>>(),
    }))
}

#[http_component]
async fn handle(req: Request) -> anyhow::Result<impl IntoResponse> {
    let t0 = Instant::now();
    let seq = INSTANCE_SEQ.fetch_add(1, Ordering::Relaxed) + 1;
    let path = req.path().to_string();
    let out = match (req.method().clone(), path.as_str()) {
        (Method::Get, "/") | (Method::Get, "/index.html") => Out { status: 200, ctype: "text/html; charset=utf-8", body: INDEX.to_string(), extra: vec![("cache-control", "no-cache".into())] },
        (Method::Get, "/api/health") => json_out(200, &json!({ "ok": true, "version": book::version_at(now_ms()) })),
        (Method::Get, "/api/meta") => meta(),
        (Method::Get, "/api/board") => board(&req).await,
        (Method::Post, "/api/quote") => quote(&req).await,
        (Method::Post, "/api/precheck") => precheck(&req).await,
        (Method::Get, "/api/simulate") => simulate(&req).await,
        (Method::Get, "/api/redirect") => {
            let p = query(&req, "path").unwrap_or_else(|| "/".into());
            redirect(&req, &p)
        }
        (Method::Get, p) if p.starts_with("/go/") => redirect(&req, p),
        (Method::Get, p) if p.starts_with("/src/") => match SOURCES.iter().find(|(n, _)| Some(*n) == p.strip_prefix("/src/")) {
            Some((_, src)) => Out { status: 200, ctype: "text/plain; charset=utf-8", body: src.to_string(), extra: vec![] },
            None => json_out(404, &json!({ "error": "no such source file" })),
        },
        _ => json_out(404, &json!({ "error": "not found" })),
    };
    let mut rb = Response::builder();
    rb.status(out.status)
        .header("content-type", out.ctype)
        .header("x-fn-us", t0.elapsed().as_micros().to_string())
        .header("x-instance-seq", seq.to_string())
        .header("access-control-expose-headers", "x-fn-us, x-instance-seq");
    for (k, v) in &out.extra {
        rb.header(*k, v.as_str());
    }
    if path.starts_with("/api/") {
        rb.header("cache-control", "no-store");
    }
    Ok(rb.body(out.body).build())
}
