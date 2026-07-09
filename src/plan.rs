//! Plan: diff rendered artifacts against the current state directory and
//! surface risks. Read-only — plan never writes anything. `apply` reuses
//! `assess` so what you saw in plan is exactly what apply gates on.

use std::net::IpAddr;
use std::path::Path;

use serde::Serialize;
use serde_json::json;

use crate::config::{Finding, GatewayConfig, Protocol, Route, RoutingMode};
use crate::render;

pub const DEFAULT_STATE_DIR: &str = "/var/lib/vpnrouter";

#[derive(Debug, Serialize)]
pub struct Change {
    pub target: &'static str,
    pub action: &'static str,
    pub path: String,
}

#[derive(Debug, Serialize)]
pub struct Risk {
    pub level: &'static str,
    pub code: &'static str,
    pub message: String,
}

pub struct Assessment {
    pub changes: Vec<Change>,
    pub risks: Vec<Risk>,
}

pub fn assess(
    cfg: &GatewayConfig,
    warnings: &[Finding],
    state_dir: &Path,
    ssh_connection: Option<&str>,
) -> Assessment {
    let resolved = crate::subscription::load_resolved(state_dir);
    let artifacts = [
        (
            "sing-box",
            "sing-box.json",
            render::render_sing_box(cfg, resolved.as_ref()),
        ),
        ("nftables", "nft.rules", render::render_nft(cfg)),
    ];
    let current_dir = state_dir.join("current");
    let mut changes = Vec::new();
    for (target, file, rendered) in &artifacts {
        let path = current_dir.join(file);
        let action = match std::fs::read_to_string(&path) {
            Err(_) => Some("create"),
            Ok(cur) if cur != *rendered => Some("replace"),
            Ok(_) => None,
        };
        if let Some(action) = action {
            changes.push(Change {
                target,
                action,
                path: path.display().to_string(),
            });
        }
    }

    let mut risks: Vec<Risk> = warnings
        .iter()
        .map(|w| Risk {
            level: "warning",
            code: w.code,
            message: w.message.clone(),
        })
        .collect();
    if resolved.is_none() {
        risks.push(Risk {
            level: "warning",
            code: "OUTBOUND_UNRESOLVED",
            message: "vpn outbound is a placeholder; run resolve-subscription so the rendered sing-box config is connectable".to_string(),
        });
    }
    if let Some(message) = ssh_risk(cfg, ssh_connection) {
        risks.push(Risk {
            level: "warning",
            code: "SSH_MAY_DROP",
            message,
        });
    }
    Assessment { changes, risks }
}

pub fn build_plan(
    cfg: &GatewayConfig,
    warnings: &[Finding],
    config_path: &Path,
    state_dir: &Path,
    ssh_connection: Option<&str>,
) -> String {
    let a = assess(cfg, warnings, state_dir, ssh_connection);
    serde_json::to_string_pretty(&json!({
        "ok": true,
        "v": crate::error::V,
        "config_path": config_path.display().to_string(),
        "changes": a.changes,
        "risks": a.risks,
    }))
    .expect("plan serializes")
}

/// Deterministic matcher: which policy handles this traffic and why.
/// Mirrors render order exactly: management -> policies (file order,
/// first-match) -> routing.mode final. No host access, no guessing.
pub fn explain(
    cfg: &GatewayConfig,
    source: std::net::IpAddr,
    dest: Option<std::net::IpAddr>,
    protocol: Option<Protocol>,
    port: Option<u16>,
) -> String {
    let mut trace: Vec<serde_json::Value> = Vec::new();
    let mut verdict: Option<(&str, &str, serde_json::Value)> = None; // route, outbound, via

    if let Some(m) = cfg.management.sources.iter().find(|m| m.contains(&source)) {
        trace.push(json!({"stage": "management", "matched": true,
            "reason": format!("source is in management source {m}")}));
        verdict = Some(("direct", "direct", json!({"via": "management"})));
    } else {
        trace.push(json!({"stage": "management", "matched": false,
            "reason": "source is not in [management] sources"}));
    }

    if verdict.is_none() {
        for p in &cfg.policies {
            let reason = if !p.source.contains(&source) {
                Some(format!("source not in {}", p.source))
            } else {
                match (p.protocol, p.port) {
                    (Some(need), _) if protocol != Some(need) => Some(format!(
                        "policy requires protocol {}, query has {}",
                        need.as_str(),
                        protocol.map_or("none", |x| x.as_str())
                    )),
                    (_, Some(need)) if port != Some(need) => Some(format!(
                        "policy requires port {need}, query has {}",
                        port.map_or("none".to_string(), |x| x.to_string())
                    )),
                    _ => None,
                }
            };
            match reason {
                Some(reason) => trace.push(json!({"stage": "policy", "name": p.name,
                    "matched": false, "reason": reason})),
                None => {
                    trace.push(json!({"stage": "policy", "name": p.name, "matched": true}));
                    let (route, outbound) = match p.route {
                        Route::Vpn => ("vpn", render::PLACEHOLDER_OUTBOUND),
                        Route::Direct => ("direct", "direct"),
                        Route::Block => ("block", "reject"),
                    };
                    verdict = Some((route, outbound, json!({"via": "policy", "name": p.name})));
                    break;
                }
            }
        }
    }

    let (route, outbound, via) = verdict.unwrap_or_else(|| match cfg.routing.mode {
        RoutingMode::Full => (
            "vpn",
            render::PLACEHOLDER_OUTBOUND,
            json!({"via": "routing.mode=full"}),
        ),
        RoutingMode::Split => ("direct", "direct", json!({"via": "routing.mode=split"})),
    });

    let mut notes: Vec<String> = vec![
        "destination is accepted but not evaluated (no destination policies in v1 schema)"
            .to_string(),
    ];
    if route == "vpn" && cfg.killswitch.enabled {
        notes.push(
            "killswitch: if the tunnel is down this traffic is dropped, not leaked to WAN"
                .to_string(),
        );
    }
    if outbound == render::PLACEHOLDER_OUTBOUND {
        notes.push("outbound is a placeholder until resolve-subscription exists".to_string());
    }

    serde_json::to_string_pretty(&json!({
        "ok": true,
        "v": crate::error::V,
        "data": {
            "query": {
                "source": source.to_string(),
                "dest": dest.map(|d| d.to_string()),
                "protocol": protocol.map(|p| p.as_str()),
                "port": port,
            },
            "verdict": { "route": route, "outbound": outbound, "decided_by": via },
            "trace": trace,
            "notes": notes,
        }
    }))
    .expect("explain serializes")
}

/// Detects whether the current SSH session's client would be routed through
/// vpn/block by this config. `ssh_connection` is the raw SSH_CONNECTION /
/// SSH_CLIENT value ("client_ip client_port ..."). UDP-only policies are
/// skipped: SSH is TCP.
pub fn ssh_risk(cfg: &GatewayConfig, ssh_connection: Option<&str>) -> Option<String> {
    let client: IpAddr = ssh_connection?.split_whitespace().next()?.parse().ok()?;
    if cfg.management.sources.iter().any(|m| m.contains(&client)) {
        return None;
    }
    let first_match = cfg
        .policies
        .iter()
        .find(|p| p.source.contains(&client) && p.protocol != Some(Protocol::Udp));
    match first_match {
        Some(p) if p.route != Route::Direct => Some(format!(
            "current SSH client {client} matches policy \"{}\" (route={}) and is not in [management] sources; applying may drop this session",
            p.name,
            p.route.as_str()
        )),
        Some(_) => None,
        None if cfg.routing.mode == RoutingMode::Full => Some(format!(
            "current SSH client {client} matches no policy and routing.mode=full sends unmatched sources through vpn; it is not in [management] sources"
        )),
        None => None,
    }
}
