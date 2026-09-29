//! The book: events, markets and prices.
//!
//! By default the book is synthetic and a pure function of the clock. Every edge
//! location computes the same prices for the same 2-second price version, so the
//! demo needs no data tier. That is a demo convenience, not an architecture claim:
//! in production the book comes from the operator's pricing feed via `feed_url`
//! (KV, NATS gateway, DBaaS API, object storage, any HTTPS JSON source).

use serde::{Deserialize, Serialize};

/// Prices reprice every 2 seconds; the version is what a bet slip quotes against.
pub const VERSION_MS: u64 = 2_000;
/// Bookmaker margin on two-way markets.
const OVERROUND: f64 = 1.045;

#[derive(Serialize, Deserialize, Clone)]
pub struct Book {
    pub version: u64,
    pub source: String,
    pub events: Vec<Event>,
}

#[derive(Serialize, Deserialize, Clone)]
pub struct Event {
    pub id: String,
    pub league: String,
    /// "game" (American odds) or "contract" (prediction market, priced in cents).
    pub kind: String,
    pub name: String,
    pub teams: Vec<String>,
    /// "live" or "pregame".
    pub status: String,
    pub clock: String,
    pub markets: Vec<Market>,
}

#[derive(Serialize, Deserialize, Clone)]
pub struct Market {
    pub id: String,
    pub name: String,
    pub suspended: bool,
    pub selections: Vec<Selection>,
}

#[derive(Serialize, Deserialize, Clone)]
pub struct Selection {
    pub id: String,
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub line: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub team: Option<String>,
    /// Fair probability behind the price.
    pub prob: f64,
    /// American odds for games, cents (1-99) for contracts.
    pub price: i32,
    /// "american" or "cents".
    pub fmt: String,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub boost: Option<Boost>,
}

#[derive(Serialize, Deserialize, Clone)]
pub struct Boost {
    pub price: i32,
    pub pct: f64,
    pub base_pct: f64,
    pub reason: String,
}

struct Spec {
    id: &'static str,
    league: &'static str,
    home: &'static str,
    away: &'static str,
    live: bool,
    start: &'static str,
    home_prob: f64,
    spread: f64,
    total: f64,
    phase: f64,
}

const GAMES: &[Spec] = &[
    Spec { id: "nfl-chi-det", league: "NFL", home: "Lions", away: "Bears", live: true, start: "", home_prob: 0.66, spread: -5.5, total: 44.5, phase: 0.3 },
    Spec { id: "mlb-lad-sd", league: "MLB", home: "Padres", away: "Dodgers", live: true, start: "", home_prob: 0.47, spread: -1.5, total: 8.5, phase: 2.1 },
    Spec { id: "nfl-kc-buf", league: "NFL", home: "Bills", away: "Chiefs", live: false, start: "Sun 4:25 PM ET", home_prob: 0.55, spread: -2.5, total: 47.5, phase: 4.0 },
];

struct ContractSpec {
    id: &'static str,
    name: &'static str,
    team: &'static str,
    prob: f64,
    phase: f64,
}

const CONTRACTS: &[ContractSpec] = &[
    ContractSpec { id: "pm-kc-afcw", name: "Chiefs win the AFC West", team: "Chiefs", prob: 0.61, phase: 1.2 },
    ContractSpec { id: "pm-det-playoffs", name: "Lions make the playoffs", team: "Lions", prob: 0.72, phase: 3.3 },
];

pub fn version_at(ms: u64) -> u64 {
    ms / VERSION_MS
}

fn drift(v: u64, phase: f64, live: bool) -> f64 {
    let x = v as f64;
    let a = if live { 0.06 } else { 0.015 };
    a * (x * 0.21 + phase).sin() + a * 0.6 * (x * 0.047 + phase * 1.7).sin()
}

fn clamp_p(p: f64) -> f64 {
    p.clamp(0.04, 0.96)
}

/// Fair probability plus margin to American odds, rounded to 5.
pub fn to_american(p: f64) -> i32 {
    let q = (p * OVERROUND).clamp(0.02, 0.98);
    let raw = if q >= 0.5 { -100.0 * q / (1.0 - q) } else { 100.0 * (1.0 - q) / q };
    let r = ((raw / 5.0).round() * 5.0) as i32;
    if r > -100 && r < 100 { if raw < 0.0 { -100 } else { 100 } } else { r }
}

/// Decimal payout multiple for either price format.
pub fn decimal(price: i32, fmt: &str) -> f64 {
    if fmt == "cents" {
        100.0 / (price.max(1) as f64)
    } else if price > 0 {
        1.0 + price as f64 / 100.0
    } else {
        1.0 + 100.0 / (price.unsigned_abs().max(1) as f64)
    }
}

pub fn american_from_decimal(d: f64) -> i32 {
    let raw = if d >= 2.0 { (d - 1.0) * 100.0 } else { -100.0 / (d - 1.0).max(0.01) };
    ((raw / 5.0).round() * 5.0) as i32
}

fn clock(spec: &Spec, ms: u64) -> String {
    let secs = (ms / 1000) % 3600;
    if spec.league == "NFL" {
        let q = 1 + secs / 900;
        let rem = 900 - secs % 900;
        format!("Q{} {:02}:{:02}", q, rem / 60, rem % 60)
    } else {
        let half = (secs / 200) % 18;
        format!("{} {}", if half % 2 == 0 { "Top" } else { "Bot" }, 1 + half / 2)
    }
}

fn sel(id: String, name: String, line: Option<f64>, team: Option<&str>, p: f64, fmt: &str) -> Selection {
    let price = if fmt == "cents" { (p * 100.0).round().clamp(1.0, 99.0) as i32 } else { to_american(p) };
    Selection { id, name, line, team: team.map(str::to_string), prob: p, price, fmt: fmt.to_string(), boost: None }
}

/// The synthetic book at a given wall-clock time.
pub fn synthetic(ms: u64) -> Book {
    let v = version_at(ms);
    let mut events = Vec::new();

    for g in GAMES {
        // Live markets suspend for 4s of every 60s: a goal, a turnover, a VAR check.
        let suspended = g.live && (v + (g.phase * 10.0) as u64) % 30 < 2;
        let ph = clamp_p(g.home_prob + drift(v, g.phase, g.live));
        let sp = clamp_p(0.5 + drift(v, g.phase + 1.0, g.live) * 0.5);
        let ov = clamp_p(0.5 + drift(v, g.phase + 2.0, g.live) * 0.5);
        let mk = |id: &str, name: &str, sels: Vec<Selection>| Market {
            id: format!("{}-{}", g.id, id),
            name: name.to_string(),
            suspended,
            selections: sels,
        };
        events.push(Event {
            id: g.id.to_string(),
            league: g.league.to_string(),
            kind: "game".into(),
            name: format!("{} @ {}", g.away, g.home),
            teams: vec![g.away.to_string(), g.home.to_string()],
            status: if g.live { "live".into() } else { "pregame".into() },
            clock: if g.live { clock(g, ms) } else { g.start.to_string() },
            markets: vec![
                mk("ml", "Moneyline", vec![
                    sel(format!("{}-ml-away", g.id), g.away.into(), None, Some(g.away), 1.0 - ph, "american"),
                    sel(format!("{}-ml-home", g.id), g.home.into(), None, Some(g.home), ph, "american"),
                ]),
                mk("spread", "Spread", vec![
                    sel(format!("{}-spread-away", g.id), g.away.into(), Some(-g.spread), Some(g.away), 1.0 - sp, "american"),
                    sel(format!("{}-spread-home", g.id), g.home.into(), Some(g.spread), Some(g.home), sp, "american"),
                ]),
                mk("total", "Total", vec![
                    sel(format!("{}-total-over", g.id), "Over".into(), Some(g.total), None, ov, "american"),
                    sel(format!("{}-total-under", g.id), "Under".into(), Some(g.total), None, 1.0 - ov, "american"),
                ]),
            ],
        });
    }

    for c in CONTRACTS {
        let p = clamp_p(c.prob + drift(v, c.phase, true) * 0.5);
        events.push(Event {
            id: c.id.to_string(),
            league: "Predictions".into(),
            kind: "contract".into(),
            name: c.name.to_string(),
            teams: vec![c.team.to_string()],
            status: "live".into(),
            clock: "Trading".into(),
            markets: vec![Market {
                id: format!("{}-yn", c.id),
                name: "Event contract".into(),
                suspended: false,
                selections: vec![
                    sel(format!("{}-yes", c.id), "Yes".into(), None, Some(c.team), p, "cents"),
                    sel(format!("{}-no", c.id), "No".into(), None, None, 1.0 - p, "cents"),
                ],
            }],
        });
    }

    Book { version: v, source: "synthetic (clock-derived)".into(), events }
}

/// Look up a selection and whether its market is suspended.
pub fn find<'a>(book: &'a Book, selection_id: &str) -> Option<(&'a Event, &'a Market, &'a Selection)> {
    for e in &book.events {
        for m in &e.markets {
            for s in &m.selections {
                if s.id == selection_id {
                    return Some((e, m, s));
                }
            }
        }
    }
    None
}
