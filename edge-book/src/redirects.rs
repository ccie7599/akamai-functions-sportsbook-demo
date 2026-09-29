//! Mass redirects, resolved inside the Function.
//!
//! Three layers, checked in order:
//! 1. Campaign short codes (`/go/ABCD2345`): 100,000 entries compiled into the
//!    component (see build.rs), found by binary search.
//! 2. Legacy URL rules: old site paths rewritten to the current structure.
//! 3. Jurisdiction: the landing page depends on where the customer is, because
//!    offers differ by state.
//!
//! Editing redirects without a redeploy is what Akamai Functions KV is for; we
//! measured ~22 ms per KV read from the Function, so it suits overrides, not the
//! hot path. Compiled-in lookups cost microseconds.

use serde::Serialize;

const CODES: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/codes.bin"));
const CAMPAIGNS: &str = include_str!(concat!(env!("OUT_DIR"), "/campaigns.txt"));
const SAMPLES: &str = include_str!(concat!(env!("OUT_DIR"), "/samples.txt"));
const REC: usize = 10;

/// States where the demo's (fictional) offer rules send the welcome offer to an
/// alternate landing page. Demo configuration, not any state's actual rules.
const ALT_WELCOME_STATES: &[&str] = &["NY", "MA"];

#[derive(Serialize)]
pub struct Resolution {
    pub input: String,
    pub matched: &'static str,
    pub status: u16,
    pub location: String,
    pub detail: String,
    pub table_size: usize,
}

pub fn table_size() -> usize {
    CODES.len() / REC
}

pub fn samples() -> Vec<&'static str> {
    SAMPLES.lines().collect()
}

fn lookup(code: &str) -> Option<&'static str> {
    let key = code.as_bytes();
    if key.len() != 8 {
        return None;
    }
    let (mut lo, mut hi) = (0usize, table_size());
    while lo < hi {
        let mid = (lo + hi) / 2;
        let rec = &CODES[mid * REC..mid * REC + REC];
        match rec[..8].cmp(key) {
            std::cmp::Ordering::Less => lo = mid + 1,
            std::cmp::Ordering::Greater => hi = mid,
            std::cmp::Ordering::Equal => {
                let idx = u16::from_le_bytes([rec[8], rec[9]]) as usize;
                return CAMPAIGNS.lines().nth(idx);
            }
        }
    }
    None
}

pub fn resolve(path: &str, state: &str) -> Resolution {
    let input = path.to_string();
    let size = table_size();

    if let Some(code) = path.strip_prefix("/go/") {
        let code = code.trim_matches('/').to_ascii_uppercase();
        return match lookup(&code) {
            Some(campaign) => {
                let alt = campaign.contains("-welcome-") && ALT_WELCOME_STATES.contains(&state);
                let landing = if alt { format!("/promos/{campaign}/{}", state.to_ascii_lowercase()) } else { format!("/promos/{campaign}") };
                Resolution {
                    input,
                    matched: "campaign code",
                    status: 302,
                    location: format!("{landing}?utm_campaign={campaign}&code={code}"),
                    detail: if alt { format!("{campaign}, {state} landing variant") } else { campaign.to_string() },
                    table_size: size,
                }
            }
            None => Resolution { input, matched: "campaign code (unknown)", status: 302, location: "/promos".into(), detail: format!("{code} not in table; sent to promos hub"), table_size: size },
        };
    }

    // Legacy site structure, e.g. /sportsbook/leagues/nfl -> /sports/nfl
    let legacy: &[(&str, &str)] = &[
        ("/sportsbook/leagues/", "/sports/"),
        ("/sportsbook/event/", "/event/"),
        ("/betslip/share/", "/slip/"),
        ("/m/", "/"),
    ];
    for (from, to) in legacy {
        if let Some(rest) = path.strip_prefix(from) {
            return Resolution { input, matched: "legacy URL rule", status: 301, location: format!("{to}{rest}"), detail: format!("{from}* → {to}*"), table_size: size };
        }
    }

    if path == "/" || path.is_empty() {
        return Resolution { input, matched: "jurisdiction", status: 302, location: format!("/{}/", state.to_ascii_lowercase()), detail: format!("customer located in {state}"), table_size: size };
    }

    Resolution { input, matched: "none", status: 404, location: String::new(), detail: "no rule matched".into(), table_size: size }
}
