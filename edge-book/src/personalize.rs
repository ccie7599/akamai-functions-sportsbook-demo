//! Per-request personalization: event ordering, dynamic odds boosts, promos.
//!
//! Everything here is computed at the edge on each request from the book plus the
//! request context. Nothing is precomputed per user, so there is nothing to cache
//! or invalidate when a price moves.

use crate::book::{self, Boost, Book};
use serde::Serialize;

pub struct Ctx {
    pub segment: String,
    pub state: String,
    pub fav: Option<String>,
}

#[derive(Serialize)]
pub struct Promo {
    pub id: String,
    pub title: String,
    pub detail: String,
}

/// Base boost a segment is offered, as a % increase in profit.
fn base_pct(segment: &str) -> f64 {
    match segment {
        "new" => 25.0,
        "lapsed" => 50.0,
        "vip" => 10.0,
        _ => 0.0,
    }
}

/// Apply personalization to `book` in place. `prev` is the book 10 seconds ago,
/// used to measure how fast the featured price is moving.
pub fn apply(book: &mut Book, prev: &Book, ctx: &Ctx) -> Vec<Promo> {
    // Favorite team's events first.
    if let Some(fav) = &ctx.fav {
        book.events.sort_by_key(|e| if e.teams.iter().any(|t| t == fav) { 0 } else { 1 });
    }

    boost(book, prev, ctx);
    promos(ctx)
}

/// Pick one featured moneyline and boost it, trimming the boost when the price is
/// moving fast. A boost is a price the operator is choosing to give away; offering
/// the full amount on a price that just moved 5 points is how a promo becomes a
/// liability.
fn boost(book: &mut Book, prev: &Book, ctx: &Ctx) {
    let pct_base = base_pct(&ctx.segment);
    if pct_base == 0.0 {
        return;
    }

    let featured_id = book
        .events
        .iter()
        .filter(|e| e.kind == "game")
        .find(|e| ctx.fav.as_ref().map_or(e.status == "live", |f| e.teams.iter().any(|t| t == f)))
        .or_else(|| book.events.iter().find(|e| e.kind == "game"))
        .and_then(|e| {
            let ml = e.markets.iter().find(|m| m.name == "Moneyline")?;
            let s = ml
                .selections
                .iter()
                .find(|s| ctx.fav.is_some() && s.team == ctx.fav)
                .unwrap_or(&ml.selections[1]);
            Some(s.id.clone())
        });
    let Some(id) = featured_id else { return };

    let prev_p = book::find(prev, &id).map(|(_, _, s)| s.prob);
    for e in book.events.iter_mut() {
        for m in e.markets.iter_mut() {
            let suspended = m.suspended;
            for s in m.selections.iter_mut().filter(|s| s.id == id) {
                if suspended {
                    s.boost = Some(Boost { price: s.price, pct: 0.0, base_pct: pct_base, reason: "Paused: market suspended".into() });
                    continue;
                }
                let moved = prev_p.map_or(0.0, |p| (s.prob - p).abs());
                let trim = (moved * 8.0).min(0.8);
                let pct = (pct_base * (1.0 - trim) * 10.0).round() / 10.0;
                let d = book::decimal(s.price, &s.fmt);
                let boosted = book::american_from_decimal(1.0 + (d - 1.0) * (1.0 + pct / 100.0));
                let reason = if trim >= 0.05 {
                    format!("{}% boost trimmed to {}%: price moved {:.1} pts in 10s", pct_base, pct, moved * 100.0)
                } else {
                    format!("{}% profit boost for {} customers", pct_base, ctx.segment)
                };
                s.boost = Some(Boost { price: boosted, pct, base_pct: pct_base, reason });
            }
        }
    }
}

/// Promo eligibility by segment and jurisdiction. The jurisdiction table is demo
/// configuration, not a statement of any state's rules.
fn promos(ctx: &Ctx) -> Vec<Promo> {
    let mut out = Vec::new();
    let seg = ctx.segment.as_str();
    let restricted_state = matches!(ctx.state.as_str(), "NY" | "MA");

    if seg == "new" && !restricted_state {
        out.push(Promo { id: "welcome".into(), title: "First bet back as a bonus bet, up to $50".into(), detail: format!("New customer offer, eligible in {}", ctx.state) });
    }
    if seg == "new" && restricted_state {
        out.push(Promo { id: "welcome-alt".into(), title: "25% profit boost on your first bet".into(), detail: format!("{} variant of the welcome offer (demo eligibility table)", ctx.state) });
    }
    if seg == "casual" || seg == "new" {
        out.push(Promo { id: "sgp".into(), title: "20% boost on any same-game parlay".into(), detail: "Today only".into() });
    }
    if seg == "vip" {
        out.push(Promo { id: "vip".into(), title: "Raised limits and priority cash-out tonight".into(), detail: "VIP host offer".into() });
    }
    if seg == "lapsed" {
        out.push(Promo { id: "winback".into(), title: "We saved you a 50% boost".into(), detail: "One bet, max stake $25".into() });
    }
    if let Some(fav) = &ctx.fav {
        out.push(Promo { id: format!("fan-{}", fav.to_lowercase()), title: format!("{} fans: boosted player props this week", fav), detail: "Picked from your favorite team".into() });
    }
    out
}
