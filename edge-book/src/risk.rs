//! Pre-trade: verify the signed quote, run deterministic checks, then score the
//! bet with a small open-weight model.
//!
//! Acceptance still belongs to the operator's regulated platform. This runs where
//! the bet enters, so the ticket that reaches that platform is already checked
//! and scored, and obviously bad slips never make the trip.

use crate::book::{self, Book};
use crate::token;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::BTreeMap;

const MODEL_JSON: &str = include_str!("../model/risk-stand-in-v1.json");

#[derive(Deserialize)]
struct Thresholds {
    fast: f64,
    review: f64,
}

#[derive(Deserialize)]
struct Model {
    name: String,
    version: String,
    note: String,
    bias: f64,
    weights: BTreeMap<String, f64>,
    thresholds: Thresholds,
}

fn model() -> Model {
    serde_json::from_str(MODEL_JSON).expect("embedded model is valid JSON")
}

#[derive(Deserialize, Clone)]
pub struct Bettor {
    #[serde(default)]
    pub sharp_score: f64,
    #[serde(default)]
    pub bets_last_10m: f64,
    #[serde(default = "default_avg")]
    pub avg_stake: f64,
}

fn default_avg() -> f64 {
    25.0
}

#[derive(Deserialize)]
pub struct PrecheckReq {
    pub quote_token: String,
    pub stake: f64,
    #[serde(default)]
    pub segment: String,
    pub bettor: Bettor,
}

#[derive(Serialize)]
struct Contribution {
    feature: String,
    value: f64,
    weight: f64,
    contribution: f64,
}

fn max_stake(segment: &str, live: bool) -> f64 {
    let base = match segment {
        "vip" => 25_000.0,
        "casual" => 1_000.0,
        _ => 500.0,
    };
    if live { base / 2.0 } else { base }
}

/// Result of evaluating one slip, without the presentation detail.
struct Eval {
    decision: &'static str,
    reject: Option<String>,
    checks: Vec<Value>,
    score: f64,
    contributions: Vec<Contribution>,
    requote: Option<(i32, u64)>,
}

fn evaluate(b: &Book, m: &Model, sel: &str, quoted_price: i32, quoted_version: u64, stake: f64, segment: &str, bettor: &Bettor) -> Option<Eval> {
    let (event, market, current) = book::find(b, sel)?;
    let live = event.status == "live";
    let mut checks = Vec::new();
    let mut reject: Option<String> = None;

    checks.push(json!({ "check": "market open", "pass": !market.suspended }));
    if market.suspended {
        reject.get_or_insert("market suspended".into());
    }

    // Stale quote: reject only if the quoted price pays more than the current one.
    let lag = b.version.saturating_sub(quoted_version);
    let price_ok = book::decimal(quoted_price, &current.fmt) <= book::decimal(current.price, &current.fmt) + 1e-9;
    if !price_ok {
        reject.get_or_insert(format!("price moved: quoted {} now {}", quoted_price, current.price));
    }
    checks.push(json!({ "check": "price still valid", "pass": price_ok, "quote_age_s": lag * book::VERSION_MS / 1000 }));

    let limit = max_stake(segment, live);
    let within = stake > 0.0 && stake <= limit;
    if !within {
        reject.get_or_insert(format!("stake outside limit (max ${:.0})", limit));
    }
    checks.push(json!({ "check": "within limits", "pass": within, "limit": limit }));

    let features: [(&str, f64); 5] = [
        ("stake_spike", (stake.max(1.0) / bettor.avg_stake.max(1.0)).ln().clamp(-2.0, 4.0)),
        ("sharp_score", bettor.sharp_score.clamp(0.0, 1.0)),
        ("price_age_s", ((lag * book::VERSION_MS / 1000) as f64).min(30.0)),
        ("live", if live { 1.0 } else { 0.0 }),
        ("velocity", bettor.bets_last_10m / 10.0),
    ];
    let mut z = m.bias;
    let mut contributions = Vec::with_capacity(features.len());
    for (name, value) in features {
        let w = m.weights.get(name).copied().unwrap_or(0.0);
        z += w * value;
        contributions.push(Contribution { feature: name.to_string(), value, weight: w, contribution: w * value });
    }
    contributions.sort_by(|a, b| b.contribution.abs().total_cmp(&a.contribution.abs()));
    let score = 1.0 / (1.0 + (-z).exp());

    let decision = match (&reject, score) {
        (Some(_), _) => "reject",
        (None, s) if s < m.thresholds.fast => "accept_fast",
        (None, s) if s < m.thresholds.review => "accept_standard",
        _ => "trader_review",
    };
    Some(Eval { decision, reject, checks, score, contributions, requote: (!price_ok).then_some((current.price, b.version)) })
}

fn route(decision: &str) -> &'static str {
    match decision {
        "reject" => "Returned to the bettor at the edge; never reaches the regulated platform",
        "accept_fast" => "Forward to the regulated platform with no in-play delay",
        "accept_standard" => "Forward to the regulated platform with the standard in-play delay",
        _ => "Forward to the regulated platform flagged for trader review",
    }
}

pub fn precheck(b: &Book, req: &PrecheckReq, now_s: u64) -> Value {
    let m = model();
    let q = match token::verify(&req.quote_token, &token::key(), now_s) {
        Ok(q) => q,
        Err(e) => {
            return json!({
                "decision": "reject",
                "route": route("reject"),
                "reasons": [e.reason()],
                "checks": [{ "check": "quote signature", "pass": false }],
            })
        }
    };
    let Some(ev) = evaluate(b, &m, &q.sel, q.price, q.ver, req.stake, &req.segment, &req.bettor) else {
        return json!({ "decision": "reject", "reasons": ["unknown selection"] });
    };
    let mut checks = vec![json!({ "check": "quote signature", "pass": true })];
    checks.extend(ev.checks);
    json!({
        "decision": ev.decision,
        "route": route(ev.decision),
        "reasons": ev.reject.into_iter().collect::<Vec<_>>(),
        "requote": ev.requote.map(|(p, v)| json!({ "price": p, "version": v })),
        "checks": checks,
        "risk": {
            "score": (ev.score * 1000.0).round() / 1000.0,
            "model": format!("{} v{}", m.name, m.version),
            "note": m.note,
            "thresholds": { "fast": m.thresholds.fast, "review": m.thresholds.review },
            "contributions": ev.contributions,
        },
    })
}

fn rng(state: &mut u64) -> f64 {
    *state ^= *state << 13;
    *state ^= *state >> 7;
    *state ^= *state << 17;
    (*state % 1_000_000) as f64 / 1_000_000.0
}

/// Score `n` synthetic slips from a mixed population against the live book.
/// Returns the decision mix and time per slip, all measured inside this call.
pub fn simulate(b: &Book, n: usize, seed: u64) -> Value {
    let m = model();
    let t0 = std::time::Instant::now();
    let sels: Vec<(String, i32, bool)> = b
        .events
        .iter()
        .flat_map(|e| e.markets.iter().flat_map(move |mk| mk.selections.iter().map(move |s| (s.id.clone(), s.price, s.fmt == "cents"))))
        .collect();
    let mut st = seed | 1;
    let mut decisions: BTreeMap<&str, usize> = BTreeMap::new();
    let mut profiles: BTreeMap<&str, BTreeMap<&str, usize>> = BTreeMap::new();
    for _ in 0..n {
        let (sel, price, cents) = &sels[(rng(&mut st) * sels.len() as f64) as usize % sels.len()];
        let r = rng(&mut st);
        let (profile, bettor, stake) = if r < 0.70 {
            ("recreational", Bettor { sharp_score: 0.05, bets_last_10m: 1.0, avg_stake: 25.0 }, 10.0 + rng(&mut st) * 40.0)
        } else if r < 0.85 {
            ("stake spike", Bettor { sharp_score: 0.15, bets_last_10m: 3.0, avg_stake: 25.0 }, 150.0 + rng(&mut st) * 300.0)
        } else if r < 0.95 {
            ("sharp", Bettor { sharp_score: 0.85, bets_last_10m: 2.0, avg_stake: 400.0 }, 300.0 + rng(&mut st) * 500.0)
        } else {
            ("stale quote", Bettor { sharp_score: 0.4, bets_last_10m: 5.0, avg_stake: 50.0 }, 50.0)
        };
        // Stale-quote bettors hold a price from ~10s ago that pays slightly more.
        let (qp, qv) = if profile == "stale quote" {
            (if *cents { (price - 3).max(1) } else if *price > 0 { price + 25 } else { price + 15 }, b.version.saturating_sub(5))
        } else {
            (*price, b.version)
        };
        if let Some(ev) = evaluate(b, &m, sel, qp, qv, stake, "casual", &bettor) {
            *decisions.entry(ev.decision).or_default() += 1;
            *profiles.entry(profile).or_default().entry(ev.decision).or_default() += 1;
        }
    }
    let us = t0.elapsed().as_micros() as f64;
    json!({
        "n": n,
        "decisions": decisions,
        "by_profile": profiles,
        "total_us": us,
        "us_per_slip": (us / n.max(1) as f64 * 100.0).round() / 100.0,
        "model": format!("{} v{}", m.name, m.version),
    })
}
