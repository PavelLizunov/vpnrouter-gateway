//! Read-only host state: status, doctor, detect-interfaces.
//! Pure checks (artifacts, drift) are testable everywhere; host probes
//! (nft, /proc, ip) degrade to honest "unknown/unsupported" — status and
//! doctor never mutate anything and never fail on a degraded probe.

use std::path::Path;

use serde::Serialize;
use serde_json::{json, Value};

use crate::config::GatewayConfig;
use crate::error::{ok_envelope, CliError};
use crate::render;

#[derive(Debug, Serialize)]
pub struct InterfaceInfo {
    pub name: String,
    pub state: String,
    pub addresses: Vec<String>,
}

pub fn parse_ip_addr(s: &str) -> Result<Vec<InterfaceInfo>, serde_json::Error> {
    #[derive(serde::Deserialize)]
    struct Link {
        ifname: String,
        #[serde(default)]
        operstate: Option<String>,
        #[serde(default)]
        addr_info: Vec<Addr>,
    }
    #[derive(serde::Deserialize)]
    struct Addr {
        #[serde(default)]
        local: Option<String>,
        #[serde(default)]
        prefixlen: Option<u8>,
    }
    let links: Vec<Link> = serde_json::from_str(s)?;
    Ok(links
        .into_iter()
        .map(|l| InterfaceInfo {
            name: l.ifname,
            state: l.operstate.unwrap_or_else(|| "unknown".to_string()),
            addresses: l
                .addr_info
                .into_iter()
                .filter_map(|a| Some(format!("{}/{}", a.local?, a.prefixlen?)))
                .collect(),
        })
        .collect())
}

pub fn detect_interfaces() -> Result<Vec<InterfaceInfo>, CliError> {
    if !cfg!(target_os = "linux") {
        return Err(CliError::env(
            "UNSUPPORTED_PLATFORM",
            "detect-interfaces shells out to `ip -j addr` and only runs on Linux".to_string(),
        ));
    }
    let out = std::process::Command::new("ip")
        .args(["-j", "addr"])
        .output()
        .map_err(|e| {
            CliError::env(
                "IP_COMMAND_FAILED",
                format!("failed to run `ip -j addr`: {e}"),
            )
        })?;
    if !out.status.success() {
        return Err(CliError::env(
            "IP_COMMAND_FAILED",
            format!("`ip -j addr` exited with {}", out.status),
        ));
    }
    parse_ip_addr(&String::from_utf8_lossy(&out.stdout)).map_err(|e| {
        CliError::env(
            "IP_OUTPUT_UNPARSEABLE",
            format!("cannot parse `ip -j addr` output: {e}"),
        )
    })
}

/// (current artifacts present, last-good present)
pub fn artifact_flags(state_dir: &Path) -> (bool, bool) {
    let all = |dir: &Path| {
        ["sing-box.json", "nft.rules"]
            .iter()
            .all(|f| dir.join(f).exists())
    };
    (
        all(&state_dir.join("current")),
        all(&state_dir.join("last-good")),
    )
}

/// None when current artifacts are absent; Some(true) when they match what
/// this config renders (i.e. config not edited since last apply).
pub fn config_in_sync(cfg: &GatewayConfig, state_dir: &Path) -> Option<bool> {
    let current = state_dir.join("current");
    let sb = std::fs::read_to_string(current.join("sing-box.json")).ok()?;
    let nft = std::fs::read_to_string(current.join("nft.rules")).ok()?;
    let resolved = crate::subscription::load_resolved(state_dir);
    Some(sb == render::render_sing_box(cfg, resolved.as_ref()) && nft == render::render_nft(cfg))
}

/// nft probe: (binary_found, table_present, error). `table_present` is None
/// when it cannot be determined (no permission, unexpected failure).
fn nft_probe() -> (bool, Option<bool>, Option<String>) {
    let out = match std::process::Command::new("nft")
        .args(["list", "table", "inet", "vpnrouter"])
        .output()
    {
        Ok(o) => o,
        Err(e) => return (false, None, Some(format!("cannot run nft: {e}"))),
    };
    if out.status.success() {
        return (true, Some(true), None);
    }
    let stderr = String::from_utf8_lossy(&out.stderr).trim().to_string();
    if stderr.contains("No such file or directory") {
        (true, Some(false), None)
    } else {
        (true, None, Some(stderr))
    }
}

fn ip_forward_enabled() -> Option<bool> {
    std::fs::read_to_string("/proc/sys/net/ipv4/ip_forward")
        .ok()
        .map(|s| s.trim() == "1")
}

pub fn cmd_status(cfg: Option<&GatewayConfig>, state_dir: &Path) -> Result<String, CliError> {
    let (current, last_good) = artifact_flags(state_dir);
    let in_sync = cfg.and_then(|c| config_in_sync(c, state_dir));
    let (nft_bin, table, nft_err) = if cfg!(target_os = "linux") {
        nft_probe()
    } else {
        (false, None, Some("unsupported platform".to_string()))
    };
    let interfaces = detect_interfaces().ok();
    let resolved = crate::subscription::load_resolved(state_dir);
    Ok(ok_envelope(json!({
        "artifacts": {
            "current": current,
            "last_good": last_good,
            "config_in_sync": in_sync,
        },
        "subscription": {
            "configured": cfg.map(|c| c.subscription.is_some()),
            "outbound_resolved": resolved.is_some(),
        },
        "nftables": {
            "binary_found": nft_bin,
            "table_present": table,
            "error": nft_err,
        },
        "interfaces": interfaces,
    })))
}

#[derive(Debug, Serialize)]
pub struct Check {
    pub name: &'static str,
    pub level: &'static str, // ok | warning | error
    pub message: String,
}

fn check(name: &'static str, level: &'static str, message: impl Into<String>) -> Check {
    Check {
        name,
        level,
        message: message.into(),
    }
}

/// Checks that need no host access — testable on any platform.
pub fn pure_doctor_checks(
    cfg: &GatewayConfig,
    warnings: &[crate::config::Finding],
    state_dir: &Path,
) -> Vec<Check> {
    let mut checks = vec![check("config", "ok", "config parses and validates")];
    for w in warnings {
        checks.push(check(
            "config_warning",
            "warning",
            format!("{}: {}", w.code, w.message),
        ));
    }
    let (current, last_good) = artifact_flags(state_dir);
    if current {
        match config_in_sync(cfg, state_dir) {
            Some(true) => checks.push(check(
                "artifacts",
                "ok",
                "current artifacts match this config",
            )),
            _ => checks.push(check(
                "artifacts",
                "warning",
                "config changed since last apply — run plan, then apply --yes",
            )),
        }
    } else {
        checks.push(check(
            "artifacts",
            "warning",
            "no current artifacts — run apply --yes",
        ));
    }
    checks.push(if last_good {
        check("rollback", "ok", "last-good present; rollback available")
    } else {
        check(
            "rollback",
            "ok",
            "no rollback point yet (apply has never replaced artifacts)",
        )
    });
    if cfg.subscription.is_some() {
        checks.push(if crate::subscription::load_resolved(state_dir).is_some() {
            check("subscription", "ok", "outbound resolved from subscription")
        } else {
            check(
                "subscription",
                "warning",
                "subscription configured but not resolved — run resolve-subscription (vpn outbound is a placeholder)",
            )
        });
    }
    checks
}

fn host_doctor_checks(cfg: &GatewayConfig) -> Vec<Check> {
    let mut checks = Vec::new();
    if !cfg!(target_os = "linux") {
        checks.push(check("host", "warning", "host probes skipped: not Linux"));
        return checks;
    }
    let (bin, table, err) = nft_probe();
    checks.push(match (bin, table) {
        (false, _) => check("nft", "error", err.unwrap_or_default()),
        (true, Some(true)) => check("nft", "ok", "table inet vpnrouter is loaded"),
        (true, Some(false)) => check(
            "nft",
            "warning",
            "table inet vpnrouter not loaded (fresh boot?) — run apply --yes",
        ),
        (true, None) => check(
            "nft",
            "warning",
            format!("cannot inspect nft state: {}", err.unwrap_or_default()),
        ),
    });
    checks.push(match ip_forward_enabled() {
        Some(true) => check("ip_forward", "ok", "net.ipv4.ip_forward = 1"),
        Some(false) => check(
            "ip_forward",
            "error",
            "net.ipv4.ip_forward = 0 — forwarded LAN traffic will not route",
        ),
        None => check(
            "ip_forward",
            "warning",
            "cannot read /proc/sys/net/ipv4/ip_forward",
        ),
    });
    match detect_interfaces() {
        Ok(ifs) => {
            for (role, name) in [("wan", &cfg.interfaces.wan), ("lan", &cfg.interfaces.lan)] {
                checks.push(match ifs.iter().find(|i| &i.name == name) {
                    Some(i) => check(
                        "interfaces",
                        "ok",
                        format!("{role} {name} found (state {})", i.state),
                    ),
                    None => check(
                        "interfaces",
                        "error",
                        format!("{role} interface {name} not found"),
                    ),
                });
            }
        }
        Err(e) => checks.push(check("interfaces", "warning", e.message)),
    }
    checks
}

pub fn cmd_doctor(
    cfg: &GatewayConfig,
    warnings: &[crate::config::Finding],
    state_dir: &Path,
) -> Result<String, CliError> {
    let mut checks = pure_doctor_checks(cfg, warnings, state_dir);
    checks.extend(host_doctor_checks(cfg));
    let out: Value = json!({
        "ok": true,
        "v": crate::error::V,
        "checks": checks,
    });
    Ok(serde_json::to_string_pretty(&out).expect("doctor serializes"))
}
