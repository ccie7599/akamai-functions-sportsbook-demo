//! Signed quote tokens: issue at the edge, verify at the edge.
//!
//! When a customer adds a price to the bet slip, the Function signs exactly what
//! was quoted (selection, price, price version, expiry). The pre-check later
//! trusts only what the signature covers, so a client that edits its price, or
//! replays an old quote, is rejected without anything reaching the core.
//!
//! Format: base64url(payload) "." base64url(HMAC-SHA256(key, payload)).

use base64::{engine::general_purpose::URL_SAFE_NO_PAD as B64, Engine};
use hmac::{Hmac, Mac};
use serde::{Deserialize, Serialize};
use sha2::Sha256;

type HmacSha256 = Hmac<Sha256>;

/// How long a quote stays valid, in seconds.
pub const TTL_S: u64 = 30;

#[derive(Serialize, Deserialize, Clone)]
pub struct Quote {
    pub sel: String,
    pub price: i32,
    pub ver: u64,
    pub exp: u64,
}

pub fn key() -> Vec<u8> {
    spin_sdk::variables::get("quote_signing_key").unwrap_or_else(|_| "dev-only-change-me".into()).into_bytes()
}

pub fn sign(q: &Quote, key: &[u8]) -> String {
    let payload = serde_json::to_vec(q).expect("quote serializes");
    let mut mac = HmacSha256::new_from_slice(key).expect("hmac accepts any key length");
    mac.update(&payload);
    format!("{}.{}", B64.encode(&payload), B64.encode(mac.finalize().into_bytes()))
}

#[derive(Debug)]
pub enum TokenError {
    Malformed,
    BadSignature,
    Expired,
}

impl TokenError {
    pub fn reason(&self) -> &'static str {
        match self {
            TokenError::Malformed => "quote token malformed",
            TokenError::BadSignature => "quote signature invalid: the slip was altered after it was quoted",
            TokenError::Expired => "quote expired: request a fresh price",
        }
    }
}

pub fn verify(token: &str, key: &[u8], now_s: u64) -> Result<Quote, TokenError> {
    let (p, s) = token.split_once('.').ok_or(TokenError::Malformed)?;
    let payload = B64.decode(p).map_err(|_| TokenError::Malformed)?;
    let sig = B64.decode(s).map_err(|_| TokenError::Malformed)?;
    let mut mac = HmacSha256::new_from_slice(key).expect("hmac accepts any key length");
    mac.update(&payload);
    mac.verify_slice(&sig).map_err(|_| TokenError::BadSignature)?;
    let q: Quote = serde_json::from_slice(&payload).map_err(|_| TokenError::Malformed)?;
    if q.exp < now_s {
        return Err(TokenError::Expired);
    }
    Ok(q)
}
