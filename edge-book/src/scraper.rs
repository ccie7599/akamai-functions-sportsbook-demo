//! Scraper handling: detect, then serve plausible but wrong prices.
//!
//! Blocking a scraper tells it to rotate. Feeding it prices that look right but
//! are consistently off makes its arbitrage and odds-screen output worthless, and
//! it has no signal to react to. The noise is keyed on the scraper's fingerprint
//! and a 30-second window, so repeated requests agree with each other and cannot
//! be averaged away.
//!
//! In production the verdict would come from Bot Manager's score passed to the
//! Function as a request header; the heuristics here stand in for that.

use crate::book::Book;

const SCRAPER_UA: &[&str] = &["python-requests", "curl/", "wget", "scrapy", "headless", "httpclient", "go-http-client", "okhttp", "aiohttp", "node-fetch"];

pub struct Verdict {
    pub signals: Vec<String>,
    pub fingerprint: u64,
}

pub fn fnv(parts: &[&str]) -> u64 {
    let mut h: u64 = 0xcbf29ce484222325;
    for p in parts {
        for b in p.bytes() {
            h ^= b as u64;
            h = h.wrapping_mul(0x100000001b3);
        }
        h ^= 0xff;
        h = h.wrapping_mul(0x100000001b3);
    }
    h
}

/// `demo_forced` is the presenter's "view as scraper" switch.
pub fn detect(ua: &str, accept_language: Option<&str>, client_ip: &str, demo_forced: bool) -> Option<Verdict> {
    let ual = ua.to_ascii_lowercase();
    let mut signals = Vec::new();
    if demo_forced {
        signals.push("presenter forced scraper view".to_string());
    }
    if let Some(tool) = SCRAPER_UA.iter().find(|t| ual.contains(*t)) {
        signals.push(format!("automation user-agent ({})", tool.trim_end_matches('/')));
    }
    if ua.is_empty() {
        signals.push("no user-agent".to_string());
    }
    if accept_language.is_none() && !demo_forced {
        signals.push("no accept-language".to_string());
    }
    if signals.is_empty() {
        return None;
    }
    Some(Verdict { signals, fingerprint: fnv(&[ua, client_ip]) })
}

/// Shift every price by a stable pseudo-random amount, nudge some lines, and
/// show suspended markets as open.
pub fn poison(book: &mut Book, fp: u64) {
    let window = (book.version / 15).to_string();
    let fps = fp.to_string();
    for e in book.events.iter_mut() {
        for m in e.markets.iter_mut() {
            m.suspended = false;
            for s in m.selections.iter_mut() {
                let h = fnv(&[&fps, &s.id, &window]);
                if s.fmt == "cents" {
                    let d = 1 + (h % 3) as i32;
                    let d = if (h >> 8) & 1 == 0 { d } else { -d };
                    s.price = (s.price + d).clamp(1, 99);
                } else {
                    let d = 5 * (1 + (h % 5) as i32);
                    let d = if (h >> 8) & 1 == 0 { d } else { -d };
                    let mut p = s.price + d;
                    if p > -100 && p < 100 {
                        p = if s.price > 0 { 100 + d.abs() } else { -100 - d.abs() };
                    }
                    s.price = p;
                }
                if let Some(line) = s.line {
                    if (h >> 16) % 4 == 0 {
                        s.line = Some(line + if (h >> 20) & 1 == 0 { 0.5 } else { -0.5 });
                    }
                }
                s.boost = None;
            }
        }
    }
}
