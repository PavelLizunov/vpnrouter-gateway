//! gateway.toml data model, loading and pure validation.
//! Portable: no host inspection here — validation only sees the config.

use std::path::Path;

use ipnet::IpNet;
use serde::{Deserialize, Serialize};

use crate::error::{CliError, Suggestion};

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GatewayConfig {
    pub interfaces: Interfaces,
    #[serde(default)]
    pub management: Management,
    pub routing: Routing,
    #[serde(default)]
    pub tun: Tun,
    #[serde(default)]
    pub policies: Vec<Policy>,
    #[serde(default)]
    pub dns: Dns,
    #[serde(default)]
    pub killswitch: Killswitch,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Interfaces {
    pub wan: String,
    pub lan: String,
    pub lan_cidr: IpNet,
}

// ponytail: ssh_port field deferred to v1 — doctor is its first real consumer;
// adding an optional TOML field later is backward-compatible.
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Management {
    #[serde(default)]
    pub sources: Vec<IpNet>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Routing {
    pub mode: RoutingMode,
}

#[derive(Debug, Clone, Copy, PartialEq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RoutingMode {
    Full,
    Split,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Tun {
    #[serde(default = "default_mtu")]
    pub mtu: u16,
}

impl Default for Tun {
    fn default() -> Self {
        Tun { mtu: default_mtu() }
    }
}

fn default_mtu() -> u16 {
    1420
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Policy {
    pub name: String,
    pub source: IpNet,
    #[serde(default)]
    pub protocol: Option<Protocol>,
    #[serde(default)]
    pub port: Option<u16>,
    pub route: Route,
}

#[derive(Debug, Clone, Copy, PartialEq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Protocol {
    Tcp,
    Udp,
}

impl Protocol {
    pub fn as_str(self) -> &'static str {
        match self {
            Protocol::Tcp => "tcp",
            Protocol::Udp => "udp",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Route {
    Vpn,
    Direct,
    Block,
}

impl Route {
    pub fn as_str(self) -> &'static str {
        match self {
            Route::Vpn => "vpn",
            Route::Direct => "direct",
            Route::Block => "block",
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Dns {
    pub mode: DnsMode,
}

impl Default for Dns {
    fn default() -> Self {
        Dns {
            mode: DnsMode::Tunneled,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DnsMode {
    Tunneled,
    Direct,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Killswitch {
    #[serde(default)]
    pub enabled: bool,
}

/// One validation error or warning; `code` is stable machine contract.
#[derive(Debug, Serialize)]
pub struct Finding {
    pub code: &'static str,
    pub message: String,
}

impl Finding {
    fn new(code: &'static str, message: String) -> Self {
        Finding { code, message }
    }
}

pub fn load(path: &Path) -> Result<GatewayConfig, CliError> {
    let text = std::fs::read_to_string(path).map_err(|e| CliError {
        exit: 2,
        code: "CONFIG_NOT_FOUND",
        message: format!("cannot read {}: {e}", path.display()),
        details: Vec::new(),
        suggestions: vec![Suggestion {
            command: "vpnrouter-gateway schema --json".to_string(),
            reason: "Inspect the expected gateway.toml schema".to_string(),
        }],
        safe_to_retry: true,
    })?;
    toml::from_str(&text).map_err(|e| CliError {
        exit: 1,
        code: "CONFIG_PARSE_ERROR",
        message: format!("{} is not a valid gateway.toml: {e}", path.display()),
        details: Vec::new(),
        suggestions: vec![Suggestion {
            command: "vpnrouter-gateway schema --json".to_string(),
            reason: "Inspect the expected gateway.toml schema".to_string(),
        }],
        safe_to_retry: true,
    })
}

/// Pure validation: (errors, warnings). Errors block plan/apply; warnings
/// surface as plan risks.
pub fn validate(cfg: &GatewayConfig) -> (Vec<Finding>, Vec<Finding>) {
    let mut errors = Vec::new();
    let mut warnings = Vec::new();

    if cfg.interfaces.wan.trim().is_empty() || cfg.interfaces.lan.trim().is_empty() {
        errors.push(Finding::new(
            "INTERFACE_NAME_EMPTY",
            "interfaces.wan and interfaces.lan must be non-empty".to_string(),
        ));
    }
    if cfg.interfaces.wan == cfg.interfaces.lan {
        errors.push(Finding::new(
            "WAN_LAN_SAME",
            format!(
                "interfaces.wan and interfaces.lan are both \"{}\"",
                cfg.interfaces.wan
            ),
        ));
    }

    if cfg.policies.is_empty() {
        errors.push(Finding::new(
            "NO_POLICIES",
            "at least one [[policies]] entry is required".to_string(),
        ));
    }

    let mut seen = std::collections::BTreeSet::new();
    for p in &cfg.policies {
        if !seen.insert(p.name.as_str()) {
            errors.push(Finding::new(
                "DUPLICATE_POLICY_NAME",
                format!("policy name \"{}\" is used more than once", p.name),
            ));
        }
        let name_ok = !p.name.is_empty()
            && p.name.len() <= 64
            && p.name
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');
        if !name_ok {
            errors.push(Finding::new(
                "POLICY_NAME_INVALID",
                format!(
                    "policy name \"{}\" must be 1-64 chars of [A-Za-z0-9_-]",
                    p.name
                ),
            ));
        }
        if p.port.is_some() && p.protocol.is_none() {
            errors.push(Finding::new(
                "PORT_WITHOUT_PROTOCOL",
                format!("policy \"{}\" sets port without protocol", p.name),
            ));
        }
        if p.port == Some(0) {
            errors.push(Finding::new(
                "PORT_INVALID",
                format!("policy \"{}\" sets port 0", p.name),
            ));
        }
        if !cfg.interfaces.lan_cidr.contains(&p.source) {
            warnings.push(Finding::new(
                "POLICY_SOURCE_OUTSIDE_LAN",
                format!(
                    "policy \"{}\" source {} is outside lan_cidr {}",
                    p.name, p.source, cfg.interfaces.lan_cidr
                ),
            ));
        }
    }

    // First-match-wins: a policy is dead if an earlier one matches a superset
    // of its traffic. This is the config mistake plan/validate exists to catch.
    for (i, p) in cfg.policies.iter().enumerate() {
        if let Some(earlier) = cfg.policies[..i].iter().find(|e| {
            e.source.contains(&p.source)
                && (e.protocol.is_none() || e.protocol == p.protocol)
                && (e.port.is_none() || e.port == p.port)
        }) {
            warnings.push(Finding::new(
                "POLICY_SHADOWED",
                format!(
                    "policy \"{}\" is never reached: earlier policy \"{}\" already matches all of its traffic",
                    p.name, earlier.name
                ),
            ));
        }
    }

    if !(1280..=1500).contains(&cfg.tun.mtu) {
        errors.push(Finding::new(
            "MTU_OUT_OF_RANGE",
            format!(
                "tun.mtu {} must be within 1280..=1500 (sing-tun drops IP fragments; >1500 blackholes PMTUD)",
                cfg.tun.mtu
            ),
        ));
    }

    let has_vpn_traffic =
        cfg.routing.mode == RoutingMode::Full || cfg.policies.iter().any(|p| p.route == Route::Vpn);
    if cfg.killswitch.enabled && !has_vpn_traffic {
        errors.push(Finding::new(
            "KILLSWITCH_WITHOUT_VPN_POLICY",
            "killswitch.enabled = true but no policy routes to vpn and routing.mode is split"
                .to_string(),
        ));
    }
    if !cfg.killswitch.enabled && has_vpn_traffic {
        warnings.push(Finding::new(
            "KILLSWITCH_DISABLED",
            "vpn-routed traffic will leak to WAN if the tunnel goes down; consider [killswitch] enabled = true".to_string(),
        ));
    }

    if cfg.management.sources.is_empty() {
        warnings.push(Finding::new(
            "NO_MANAGEMENT_BYPASS",
            "no [management] sources configured; a bad apply can lock you out of SSH — add your admin host CIDR".to_string(),
        ));
    }
    for m in &cfg.management.sources {
        if let Some(p) = cfg
            .policies
            .iter()
            .find(|p| p.route == Route::Block && p.source.contains(m))
        {
            warnings.push(Finding::new(
                "MANAGEMENT_SOURCE_BLOCKED",
                format!(
                    "management source {m} is matched by block policy \"{}\"; management bypass wins, but this looks like a mistake",
                    p.name
                ),
            ));
        }
    }

    (errors, warnings)
}
