//! Subscription resolution: fetch -> decode -> parse -> select active
//! outbound -> cache. The parser is pure (fixture-testable); the network is a
//! thin Fetcher seam. Lesson from the desktop SubscriptionFetcher: the real
//! world is messy — accept base64/plain/JSON, never panic on a bad line, drop
//! what we can't parse and count it.
//!
//! Scope (ponytail): vless:// share links (the doc's primary, Reality) and
//! sing-box JSON passthrough. hysteria2/tuic/ss share links are deferred until
//! a real subscription needs them — parse_uri returns Unsupported for now.

use std::path::Path;

use serde_json::{json, Map, Value};

pub const CACHE_FILE: &str = "subscription.json";

#[derive(Debug)]
pub struct SubError(pub String);

impl SubError {
    fn new(s: impl Into<String>) -> Self {
        SubError(s.into())
    }
}

/// A parsed proxy outbound plus its display name (the tag used for selection).
#[derive(Debug, Clone)]
pub struct ResolvedOutbound {
    pub name: String,
    pub outbound: Value,
}

/// A subscription entry we recognised but can't build yet (unsupported
/// protocol). Surfaced so a dropped node is never a silent cap.
#[derive(Debug, Clone)]
pub struct Skipped {
    pub name: String,
    pub scheme: String,
}

#[derive(Debug, Default)]
pub struct ParseResult {
    pub outbounds: Vec<ResolvedOutbound>,
    pub skipped: Vec<Skipped>,
}

/// Network seam; the only thing tests replace.
pub trait Fetcher {
    fn get(&self, url: &str) -> Result<String, String>;
}

pub struct RealFetcher;

impl Fetcher for RealFetcher {
    fn get(&self, url: &str) -> Result<String, String> {
        ureq::get(url)
            .call()
            .map_err(|e| format!("fetch failed: {e}"))?
            .body_mut()
            .read_to_string()
            .map_err(|e| format!("reading response body failed: {e}"))
    }
}

/// Parse a subscription body into proxy outbounds. Accepts, in order:
/// a JSON wrapper (`{"config":"base64…"}`, e.g. ninitux), sing-box JSON
/// (`{...}` with an `outbounds` array), base64 of a URI list, or a plain
/// newline-separated URI list.
pub fn parse_subscription(body: &str) -> Result<ParseResult, SubError> {
    parse_body(body, 0)
}

fn parse_body(body: &str, depth: u8) -> Result<ParseResult, SubError> {
    if depth > 3 {
        return Err(SubError::new("subscription nesting too deep"));
    }
    let trimmed = body.trim();
    if trimmed.is_empty() {
        return Err(SubError::new("empty subscription body"));
    }
    if trimmed.starts_with('{') || trimmed.starts_with('[') {
        // Panel wrapper: unwrap a base64/plain body carried in a string field.
        if let Ok(Value::Object(m)) = serde_json::from_str::<Value>(trimmed) {
            for key in ["config", "data", "subscription"] {
                if let Some(inner) = m.get(key).and_then(|v| v.as_str()) {
                    return parse_body(inner, depth + 1);
                }
            }
        }
        return parse_singbox_json(trimmed);
    }
    // Try base64; use the decoded text only if it looks like a URI list.
    let text = match base64_decode(trimmed) {
        Some(bytes) => match String::from_utf8(bytes) {
            Ok(s) if s.contains("://") => s,
            _ => trimmed.to_string(),
        },
        None => trimmed.to_string(),
    };
    let mut out = Vec::new();
    let mut skipped = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        match parse_uri(line) {
            Ok(Some(r)) => out.push(r),
            Ok(None) => skipped.push(skipped_entry(line)),
            Err(_) => skipped.push(skipped_entry(line)),
        }
    }
    if out.is_empty() {
        return Err(SubError::new(format!(
            "no supported outbounds found ({} unsupported/unparseable entries; supports vless:// and sing-box JSON)",
            skipped.len()
        )));
    }
    Ok(ParseResult {
        outbounds: out,
        skipped,
    })
}

/// Best-effort (name, scheme) for an entry we can't build.
fn skipped_entry(uri: &str) -> Skipped {
    let scheme = uri.split_once("://").map_or("?", |(s, _)| s).to_string();
    let name = uri
        .rsplit_once('#')
        .map(|(_, f)| percent_decode(f))
        .unwrap_or_default();
    Skipped { name, scheme }
}

fn parse_singbox_json(text: &str) -> Result<ParseResult, SubError> {
    let v: Value =
        serde_json::from_str(text).map_err(|e| SubError::new(format!("invalid JSON: {e}")))?;
    // Accept either a full config ({"outbounds":[...]}) or a bare array.
    let arr = if let Some(a) = v.get("outbounds").and_then(|o| o.as_array()) {
        a.clone()
    } else if let Some(a) = v.as_array() {
        a.clone()
    } else {
        return Err(SubError::new("JSON has no outbounds array"));
    };
    const NON_PROXY: [&str; 5] = ["direct", "block", "dns", "selector", "urltest"];
    let mut out = Vec::new();
    for ob in arr {
        let ty = ob.get("type").and_then(|t| t.as_str()).unwrap_or("");
        if ty.is_empty() || NON_PROXY.contains(&ty) {
            continue;
        }
        let name = ob
            .get("tag")
            .and_then(|t| t.as_str())
            .unwrap_or(ty)
            .to_string();
        out.push(ResolvedOutbound { name, outbound: ob });
    }
    if out.is_empty() {
        return Err(SubError::new("sing-box JSON has no proxy outbounds"));
    }
    Ok(ParseResult {
        outbounds: out,
        skipped: Vec::new(),
    })
}

/// Ok(Some) = parsed; Ok(None) = recognised but unsupported scheme.
fn parse_uri(uri: &str) -> Result<Option<ResolvedOutbound>, SubError> {
    let (scheme, _) = uri
        .split_once("://")
        .ok_or_else(|| SubError::new("no scheme"))?;
    match scheme {
        "vless" => parse_vless(uri).map(Some),
        // ponytail: add when a real subscription carries them.
        "hysteria2" | "hy2" | "tuic" | "ss" | "vmess" | "trojan" => Ok(None),
        _ => Ok(None),
    }
}

/// vless://UUID@host:port?params#name  ->  sing-box vless outbound.
fn parse_vless(uri: &str) -> Result<ResolvedOutbound, SubError> {
    let rest = &uri["vless://".len()..];
    let (main, fragment) = match rest.split_once('#') {
        Some((m, f)) => (m, percent_decode(f)),
        None => (rest, String::new()),
    };
    let (userinfo_host, query) = match main.split_once('?') {
        Some((mh, q)) => (mh, q),
        None => (main, ""),
    };
    let (uuid, hostport) = userinfo_host
        .split_once('@')
        .ok_or_else(|| SubError::new("vless: missing uuid@host"))?;
    let (host, port) = split_host_port(hostport)?;
    let params = parse_query(query);
    let get = |k: &str| q(&params, k);

    let mut ob = Map::new();
    ob.insert("type".into(), json!("vless"));
    let name = if fragment.is_empty() {
        format!("{host}:{port}")
    } else {
        fragment
    };
    ob.insert("tag".into(), json!(name));
    ob.insert("server".into(), json!(host));
    ob.insert("server_port".into(), json!(port));
    ob.insert("uuid".into(), json!(uuid));
    if let Some(flow) = get("flow").filter(|f| !f.is_empty()) {
        ob.insert("flow".into(), json!(flow));
    }
    // xudp packet encoding keeps UDP (games/voice) working over vless.
    ob.insert("packet_encoding".into(), json!("xudp"));

    let security = get("security").unwrap_or("");
    if security == "tls" || security == "reality" {
        let mut tls = Map::new();
        tls.insert("enabled".into(), json!(true));
        let sni = get("sni").or_else(|| get("peer")).unwrap_or(host);
        tls.insert("server_name".into(), json!(sni));
        if let Some(alpn) = get("alpn").filter(|a| !a.is_empty()) {
            tls.insert(
                "alpn".into(),
                json!(alpn.split(',').map(|s| s.trim()).collect::<Vec<_>>()),
            );
        }
        if let Some(fp) = get("fp").filter(|f| !f.is_empty()) {
            tls.insert("utls".into(), json!({ "enabled": true, "fingerprint": fp }));
        } else if security == "reality" {
            // Reality requires uTLS; default to a common fingerprint.
            tls.insert(
                "utls".into(),
                json!({ "enabled": true, "fingerprint": "chrome" }),
            );
        }
        if security == "reality" {
            let mut reality = Map::new();
            reality.insert("enabled".into(), json!(true));
            if let Some(pbk) = get("pbk") {
                reality.insert("public_key".into(), json!(pbk));
            }
            reality.insert("short_id".into(), json!(get("sid").unwrap_or("")));
            tls.insert("reality".into(), Value::Object(reality));
        }
        ob.insert("tls".into(), Value::Object(tls));
    }

    if let Some(t) = transport(&params) {
        ob.insert("transport".into(), t);
    }

    Ok(ResolvedOutbound {
        name: ob["tag"].as_str().unwrap().to_string(),
        outbound: Value::Object(ob),
    })
}

/// Look up a query param value.
fn q<'a>(params: &'a [(String, String)], key: &str) -> Option<&'a str> {
    params
        .iter()
        .find(|(pk, _)| pk == key)
        .map(|(_, v)| v.as_str())
}

fn transport(params: &[(String, String)]) -> Option<Value> {
    let nonempty = |k: &str| q(params, k).filter(|v| !v.is_empty());
    match q(params, "type").unwrap_or("tcp") {
        "ws" => {
            let mut t = json!({ "type": "ws" });
            if let Some(p) = nonempty("path") {
                t["path"] = json!(percent_decode(p));
            }
            if let Some(h) = nonempty("host") {
                t["headers"] = json!({ "Host": h });
            }
            Some(t)
        }
        "grpc" => {
            let mut t = json!({ "type": "grpc" });
            if let Some(s) = nonempty("serviceName") {
                t["service_name"] = json!(percent_decode(s));
            }
            Some(t)
        }
        "http" => {
            let mut t = json!({ "type": "http" });
            if let Some(p) = nonempty("path") {
                t["path"] = json!(percent_decode(p));
            }
            if let Some(h) = nonempty("host") {
                t["host"] = json!([h]);
            }
            Some(t)
        }
        // tcp / quic / unknown -> no transport block (sing-box default tcp).
        _ => None,
    }
}

fn split_host_port(hp: &str) -> Result<(&str, u16), SubError> {
    // IPv6 literal [::1]:443
    if let Some(rest) = hp.strip_prefix('[') {
        let (h, p) = rest
            .split_once("]:")
            .ok_or_else(|| SubError::new("bad IPv6 host:port"))?;
        return Ok((h, p.parse().map_err(|_| SubError::new("bad port"))?));
    }
    let (h, p) = hp
        .rsplit_once(':')
        .ok_or_else(|| SubError::new("missing :port"))?;
    Ok((h, p.parse().map_err(|_| SubError::new("bad port"))?))
}

fn parse_query(q: &str) -> Vec<(String, String)> {
    q.split('&')
        .filter(|s| !s.is_empty())
        .map(|pair| match pair.split_once('=') {
            Some((k, v)) => (k.to_string(), percent_decode(v)),
            None => (pair.to_string(), String::new()),
        })
        .collect()
}

/// Decode %XX escapes. '+' is left as-is (vless names use %20, not '+').
fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let (Some(h), Some(l)) = (hexval(bytes[i + 1]), hexval(bytes[i + 2])) {
                out.push(h * 16 + l);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn hexval(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

/// Tolerant base64: accepts standard and url-safe alphabets, ignores
/// whitespace and padding. Returns None if a non-alphabet byte is seen.
pub fn base64_decode(s: &str) -> Option<Vec<u8>> {
    let mut bits: u32 = 0;
    let mut nbits = 0;
    let mut out = Vec::new();
    for &b in s.as_bytes() {
        let v = match b {
            b'A'..=b'Z' => b - b'A',
            b'a'..=b'z' => b - b'a' + 26,
            b'0'..=b'9' => b - b'0' + 52,
            b'+' | b'-' => 62,
            b'/' | b'_' => 63,
            b'=' | b'\n' | b'\r' | b'\t' | b' ' => continue,
            _ => return None,
        };
        bits = (bits << 6) | v as u32;
        nbits += 6;
        if nbits >= 8 {
            nbits -= 8;
            out.push((bits >> nbits) as u8);
        }
    }
    Some(out)
}

/// Select the outbound named `active`; error lists what's available.
pub fn select<'a>(
    outbounds: &'a [ResolvedOutbound],
    active: &str,
) -> Result<&'a ResolvedOutbound, SubError> {
    outbounds.iter().find(|o| o.name == active).ok_or_else(|| {
        let names: Vec<&str> = outbounds.iter().map(|o| o.name.as_str()).collect();
        SubError::new(format!(
            "active outbound \"{active}\" not found; available: {}",
            names.join(", ")
        ))
    })
}

/// Persist the selected outbound so render/apply can use it. Real secrets are
/// kept (root-only dir); callers redact for display.
pub fn save_cache(
    state_dir: &Path,
    source_url: &str,
    chosen: &ResolvedOutbound,
) -> std::io::Result<()> {
    std::fs::create_dir_all(state_dir)?;
    let doc = json!({
        "v": 1,
        "source": crate::redact::redact_url(source_url),
        "active": chosen.name,
        "outbound": chosen.outbound,
    });
    std::fs::write(
        state_dir.join(CACHE_FILE),
        serde_json::to_string_pretty(&doc).expect("cache serializes"),
    )
}

/// The cached outbound object (untagged), or None if no subscription resolved.
pub fn load_resolved(state_dir: &Path) -> Option<Value> {
    let text = std::fs::read_to_string(state_dir.join(CACHE_FILE)).ok()?;
    let doc: Value = serde_json::from_str(&text).ok()?;
    doc.get("outbound").cloned()
}
