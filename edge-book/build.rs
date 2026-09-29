//! Generates the redirect table compiled into the Function.
//!
//! 100,000 campaign and affiliate short codes, each pointing at one of a few
//! hundred landing pages. Real operators have tables like this (podcast codes,
//! affiliate links, printed QR codes, legacy URLs); keeping it in the Function
//! means a redirect never costs an origin round trip.
//!
//! Layout of `codes.bin`: sorted fixed-width records, 8 bytes of code followed by
//! a little-endian u16 campaign index. Lookup is a binary search over the slice.

use std::{env, fs, path::Path};

pub const CODES: usize = 100_000;
const ALPHABET: &[u8] = b"ABCDEFGHJKLMNPQRSTUVWXYZ23456789";

fn splitmix(state: &mut u64) -> u64 {
    *state = state.wrapping_add(0x9E3779B97F4A7C15);
    let mut z = *state;
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58476D1CE4E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D049BB133111EB);
    z ^ (z >> 31)
}

fn main() {
    let leagues = ["nfl", "mlb", "nba", "nhl", "ncaaf", "soccer"];
    let offers = ["welcome", "boost", "parlay", "refer", "reactivate"];
    let channels = ["podcast", "social", "tv", "affiliate", "print", "email", "creator", "search"];

    let mut campaigns = Vec::new();
    for l in leagues {
        for o in offers {
            for c in channels {
                campaigns.push(format!("{l}-{o}-{c}"));
            }
        }
    }

    let mut state = 0x5EED_u64;
    let mut records: Vec<([u8; 8], u16)> = Vec::with_capacity(CODES);
    let mut seen = std::collections::HashSet::with_capacity(CODES);
    while records.len() < CODES {
        let mut code = [0u8; 8];
        for b in code.iter_mut() {
            *b = ALPHABET[(splitmix(&mut state) % ALPHABET.len() as u64) as usize];
        }
        if seen.insert(code) {
            let camp = (splitmix(&mut state) % campaigns.len() as u64) as u16;
            records.push((code, camp));
        }
    }
    records.sort_by(|a, b| a.0.cmp(&b.0));

    let mut bin = Vec::with_capacity(CODES * 10);
    for (code, camp) in &records {
        bin.extend_from_slice(code);
        bin.extend_from_slice(&camp.to_le_bytes());
    }

    let out = env::var("OUT_DIR").unwrap();
    fs::write(Path::new(&out).join("codes.bin"), bin).unwrap();
    fs::write(Path::new(&out).join("campaigns.txt"), campaigns.join("\n")).unwrap();
    // A handful of real codes from the table, for the demo UI.
    let samples: Vec<String> = records.iter().step_by(CODES / 6).map(|(c, _)| String::from_utf8_lossy(c).into_owned()).collect();
    fs::write(Path::new(&out).join("samples.txt"), samples.join("\n")).unwrap();
    println!("cargo:rerun-if-changed=build.rs");
}
