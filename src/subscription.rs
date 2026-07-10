//! Subscription resolution: fetch -> decode -> parse -> select active
//! outbound -> cache. The parser is pure (fixture-testable); the network is a
//! thin Fetcher seam. Lesson from the desktop SubscriptionFetcher: the real
//! world is messy — accept base64/plain/JSON, never panic on a bad line, drop
//! what we can't parse and count it.
//!
//! Scope: vless:// (Reality/TLS, ws/grpc/http transports), hysteria2:// (QUIC),
//! and sing-box JSON passthrough. tuic/ss/vmess/trojan/naive are surfaced as
//! skipped-with-reason until a real subscription needs them.

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

/// A subscription entry we recognised but did not emit — unsupported protocol
/// or an unsafe node (plaintext / reality-without-pbk). Surfaced with a reason
/// so a dropped node is never a silent cap.
#[derive(Debug, Clone)]
pub struct Skipped {
    pub name: String,
    pub scheme: String,
    pub reason: String,
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

/// A hung subscription server must not hang the CLI (review finding F1).
const FETCH_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(15);

impl Fetcher for RealFetcher {
    fn get(&self, url: &str) -> Result<String, String> {
        fetch_with_timeout(url, FETCH_TIMEOUT)
    }
}

pub(crate) fn fetch_with_timeout(
    url: &str,
    timeout: std::time::Duration,
) -> Result<String, String> {
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(timeout))
        .build()
        .into();
    agent
        .get(url)
        .call()
        .map_err(|e| format!("fetch failed: {e}"))?
        .body_mut()
        .read_to_string()
        .map_err(|e| format!("reading response body failed: {e}"))
}

/// Parse a subscription body into proxy outbounds. Accepts, in order:
/// a JSON wrapper (`{"config":"base64…"}`, e.g. ninitux), sing-box JSON
/// (`{...}` with an `outbounds` array), base64 of a URI list, or a plain
/// newline-separated URI list.
pub fn parse_subscription(body: &str) -> Result<ParseResult, SubError> {
    let mut result = parse_body(body, 0)?;
    // Node-safety: never emit a plaintext or malformed-reality outbound — that
    // would be clear egress from the origin IP, against the consumer's invariant
    // and the tool's own killswitch spirit. Filter here, before any downstream
    // urltest group references a tag we then drop.
    let mut safe = Vec::with_capacity(result.outbounds.len());
    for r in std::mem::take(&mut result.outbounds) {
        match node_safety_reason(&r.outbound) {
            Some(reason) => result.skipped.push(Skipped {
                name: r.name,
                scheme: "vless".to_string(),
                reason,
            }),
            None => safe.push(r),
        }
    }
    if safe.is_empty() {
        return Err(SubError::new(format!(
            "no usable outbounds: all {} recognised node(s) were unsupported or unsafe (plaintext / reality without pbk)",
            result.skipped.len()
        )));
    }
    result.outbounds = safe;
    Ok(result)
}

/// vless-only: a node we would emit as a plaintext or malformed-reality outbound
/// is unsafe. ss/trojan/hysteria2 carry their own transport crypto, so this is
/// deliberately scoped to vless (the only protocol the parser builds today).
fn node_safety_reason(ob: &Value) -> Option<String> {
    if ob.get("type").and_then(|t| t.as_str()) != Some("vless") {
        return None;
    }
    let tls = ob.get("tls");
    let tls_on = tls
        .and_then(|t| t.get("enabled"))
        .and_then(serde_json::Value::as_bool)
        == Some(true);
    if !tls_on {
        return Some("plaintext outbound (security is neither tls nor reality)".to_string());
    }
    if let Some(reality) = tls.and_then(|t| t.get("reality")) {
        let pbk = reality
            .get("public_key")
            .and_then(|p| p.as_str())
            .unwrap_or("");
        if pbk.is_empty() {
            return Some("reality outbound without public_key (pbk)".to_string());
        }
    }
    None
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

/// Best-effort (name, scheme, reason) for an entry we can't build.
fn skipped_entry(uri: &str) -> Skipped {
    let scheme = uri.split_once("://").map_or("?", |(s, _)| s).to_string();
    let name = uri
        .rsplit_once('#')
        .map(|(_, f)| percent_decode(f))
        .unwrap_or_default();
    let reason = format!("unsupported protocol {scheme}");
    Skipped {
        name,
        scheme,
        reason,
    }
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
        "hysteria2" | "hy2" => parse_hysteria2(uri).map(Some),
        // ponytail: add when a real subscription carries them.
        "tuic" | "ss" | "vmess" | "trojan" | "naive+https" | "naive" => Ok(None),
        _ => Ok(None),
    }
}

/// hysteria2://<password>@host:port[/]?sni=&obfs=salamander&obfs-password=&insecure=1#name
/// -> sing-box hysteria2 outbound. QUIC/UDP-native; always TLS (no plaintext
/// variant), so node-safety leaves it alone. Format verified against a live
/// ninitux node (note the trailing `/` in host:port and insecure=1).
fn parse_hysteria2(uri: &str) -> Result<ResolvedOutbound, SubError> {
    let rest = uri.split_once("://").map(|(_, r)| r).unwrap_or(uri);
    let (main, fragment) = match rest.split_once('#') {
        Some((m, f)) => (m, percent_decode(f)),
        None => (rest, String::new()),
    };
    let (userinfo_host, query) = match main.split_once('?') {
        Some((mh, q)) => (mh, q),
        None => (main, ""),
    };
    let (password, hostport) = userinfo_host
        .split_once('@')
        .ok_or_else(|| SubError::new("hysteria2: missing password@host"))?;
    // Panels emit host:port/ (a path); drop it before host:port parsing.
    let hostport = hostport.split('/').next().unwrap_or(hostport);
    let (host, port) = split_host_port(hostport)?;
    let params = parse_query(query);
    let get = |k: &str| q(&params, k);

    let name = if fragment.is_empty() {
        format!("{host}:{port}")
    } else {
        fragment
    };
    let mut ob = Map::new();
    ob.insert("type".into(), json!("hysteria2"));
    ob.insert("tag".into(), json!(name.clone()));
    ob.insert("server".into(), json!(host));
    ob.insert("server_port".into(), json!(port));
    ob.insert("password".into(), json!(percent_decode(password)));

    let mut tls = Map::new();
    tls.insert("enabled".into(), json!(true));
    tls.insert(
        "server_name".into(),
        json!(get("sni").or_else(|| get("peer")).unwrap_or(host)),
    );
    if matches!(get("insecure"), Some("1" | "true")) {
        tls.insert("insecure".into(), json!(true));
    }
    let alpn = get("alpn")
        .filter(|a| !a.is_empty())
        .map(|a| {
            a.split(',')
                .map(|s| s.trim().to_string())
                .collect::<Vec<_>>()
        })
        .unwrap_or_else(|| vec!["h3".to_string()]);
    tls.insert("alpn".into(), json!(alpn));
    ob.insert("tls".into(), Value::Object(tls));

    if let Some(obfs_type) = get("obfs").filter(|o| !o.is_empty()) {
        let mut obfs = Map::new();
        obfs.insert("type".into(), json!(obfs_type));
        if let Some(pw) = get("obfs-password") {
            obfs.insert("password".into(), json!(percent_decode(pw)));
        }
        ob.insert("obfs".into(), Value::Object(obfs));
    }

    Ok(ResolvedOutbound {
        name,
        outbound: Value::Object(ob),
    })
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
    ob.insert("tag".into(), json!(name.clone()));
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
        name,
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

/// Write a cache file 0600 — it holds real uuids. On non-unix (dev) the mode is
/// a no-op; the target is Linux where the file lives in a root-only state dir.
fn write_cache(path: &Path, content: &str) -> std::io::Result<()> {
    std::fs::write(path, content)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
    }
    Ok(())
}

/// Persist a single selected outbound (cache v1 = pinned case).
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
    write_cache(
        &state_dir.join(CACHE_FILE),
        &serde_json::to_string_pretty(&doc).expect("cache serializes"),
    )
}

/// Persist all resolved outbounds (cache v2 = urltest needs the whole pool).
pub fn save_cache_all(
    state_dir: &Path,
    source_url: &str,
    active: Option<&str>,
    outbounds: &[ResolvedOutbound],
) -> std::io::Result<()> {
    std::fs::create_dir_all(state_dir)?;
    let obs: Vec<&Value> = outbounds.iter().map(|o| &o.outbound).collect();
    let doc = json!({
        "v": 2,
        "source": crate::redact::redact_url(source_url),
        "active": active,
        "outbounds": obs,
    });
    write_cache(
        &state_dir.join(CACHE_FILE),
        &serde_json::to_string_pretty(&doc).expect("cache serializes"),
    )
}

/// Loaded cache, version-normalised. `outbounds` is the full pool (one entry for
/// v1/pinned); `active` is the recorded selection if any.
pub struct CachedSub {
    pub active: Option<String>,
    pub outbounds: Vec<Value>,
}

/// Read the cache. Absent -> Ok(None). Unknown version -> loud Err (so a stale
/// or foreign cache is a clear "re-run resolve-subscription", not a silent miss).
pub fn load_cache(state_dir: &Path) -> Result<Option<CachedSub>, SubError> {
    let text = match std::fs::read_to_string(state_dir.join(CACHE_FILE)) {
        Ok(t) => t,
        Err(_) => return Ok(None),
    };
    let doc: Value = serde_json::from_str(&text)
        .map_err(|e| SubError::new(format!("cache is not valid JSON: {e}")))?;
    let active = doc.get("active").and_then(|a| a.as_str()).map(String::from);
    match doc.get("v").and_then(serde_json::Value::as_u64) {
        Some(1) => {
            let ob = doc
                .get("outbound")
                .cloned()
                .ok_or_else(|| SubError::new("v1 cache missing 'outbound'".to_string()))?;
            Ok(Some(CachedSub {
                active,
                outbounds: vec![ob],
            }))
        }
        Some(2) => {
            let outbounds = doc
                .get("outbounds")
                .and_then(|o| o.as_array())
                .cloned()
                .unwrap_or_default();
            Ok(Some(CachedSub { active, outbounds }))
        }
        _ => Err(SubError::new(
            "unrecognized subscription cache version; re-run resolve-subscription".to_string(),
        )),
    }
}

/// The single pinned outbound for gateway/pinned render (active, else first).
/// Resilient: any cache problem -> None, so gateway falls back to the
/// placeholder. Proxy paths call load_cache directly for the loud channel.
pub fn load_resolved(state_dir: &Path) -> Option<Value> {
    let cached = load_cache(state_dir).ok()??;
    if let Some(active) = &cached.active {
        if let Some(found) = cached
            .outbounds
            .iter()
            .find(|o| o.get("tag").and_then(|t| t.as_str()) == Some(active.as_str()))
        {
            return Some(found.clone());
        }
    }
    cached.outbounds.first().cloned()
}
