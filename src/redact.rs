//! Secret redaction for anything printed to the user (resolve-subscription
//! output, future doctor bundle). Design borrowed from the desktop app's
//! DiagnosticsRedactor: structured, keys preserved / values masked, and the
//! non-obvious fix it learned the hard way — a secret key masks even a numeric
//! value, so an all-digit token can't slip through as "just a number".
//!
//! On-disk artifacts (/var/lib/vpnrouter, root-only) keep real secrets;
//! redaction is a display concern only.

use serde_json::{Map, Value};

/// Keys whose value is a credential regardless of type. Reality `public_key`
/// (pbk) is public by design and intentionally absent.
const SECRET_KEYS: &[&str] = &[
    "uuid",
    "password",
    "private_key",
    "pre_shared_key",
    "psk",
    "obfs_password",
    "auth",
    "auth_str",
    "token",
    "secret",
    "short_id",
];

/// Keys holding a URL: keep scheme://host, drop path/query/userinfo (token).
const URL_KEYS: &[&str] = &["url"];

const MASK: &str = "***";

pub fn redact_value(v: &Value) -> Value {
    match v {
        Value::Object(m) => {
            let mut out = Map::with_capacity(m.len());
            for (k, val) in m {
                let lk = k.to_ascii_lowercase();
                let redacted = if SECRET_KEYS.contains(&lk.as_str()) && !val.is_null() {
                    Value::String(MASK.to_string())
                } else if URL_KEYS.contains(&lk.as_str()) {
                    match val.as_str() {
                        Some(s) => Value::String(redact_url(s)),
                        None => redact_value(val),
                    }
                } else {
                    redact_value(val)
                };
                out.insert(k.clone(), redacted);
            }
            Value::Object(out)
        }
        Value::Array(a) => Value::Array(a.iter().map(redact_value).collect()),
        other => other.clone(),
    }
}

/// scheme://host[:port] only. Anything unparseable collapses to "***" rather
/// than risk leaking a token in a malformed URL.
pub fn redact_url(url: &str) -> String {
    let Some((scheme, rest)) = url.split_once("://") else {
        return MASK.to_string();
    };
    // Strip userinfo (user:pass@) then take up to the first '/', '?' or '#'.
    let after_at = rest.rsplit_once('@').map_or(rest, |(_, h)| h);
    let host: String = after_at
        .chars()
        .take_while(|&c| c != '/' && c != '?' && c != '#')
        .collect();
    if host.is_empty() {
        MASK.to_string()
    } else {
        format!("{scheme}://{host}/…")
    }
}
