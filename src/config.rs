//! gateway.toml data model, loading and pure validation.
//! Portable: no host inspection here — validation only sees the config.
//!
//! Two modes share this model: `gateway` (default; TUN L3 edge + nftables) and
//! `proxy` (mixed inbound, authoring-only, no host mutation). Absent `mode` =
//! gateway, so existing configs are unaffected. The six gateway sections are
//! `Option` so proxy mode can *reject* their presence (serde can't tell an
//! omitted `[dns]` from a defaulted one — Options make presence first-class).

use std::path::Path;

use ipnet::IpNet;
use serde::{Deserialize, Serialize};

use crate::error::{CliError, Suggestion};

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GatewayConfig {
    #[serde(default)]
    pub mode: Mode,
    #[serde(default)]
    pub proxy: Option<Proxy>,
    #[serde(default)]
    pub interfaces: Option<Interfaces>,
    #[serde(default)]
    pub routing: Option<Routing>,
    #[serde(default)]
    pub tun: Option<Tun>,
    #[serde(default)]
    pub dns: Option<Dns>,
    #[serde(default)]
    pub killswitch: Option<Killswitch>,
    #[serde(default)]
    pub management: Option<Management>,
    #[serde(default)]
    pub subscription: Option<Subscription>,
    #[serde(default)]
    pub policies: Vec<Policy>,
}

/// Gateway-mode accessors. `.expect()` here is an internal invariant: validate()
/// guarantees the section is present in gateway mode and returns early otherwise,
/// and every gateway render/plan path runs only after validate() — proxy mode
/// dispatches elsewhere. A panic here would mean a dispatch bug, not bad input.
impl GatewayConfig {
    pub fn interfaces(&self) -> &Interfaces {
        self.interfaces
            .as_ref()
            .expect("gateway mode validated to have [interfaces]")
    }
    pub fn routing_mode(&self) -> RoutingMode {
        self.routing
            .as_ref()
            .expect("gateway mode validated to have [routing]")
            .mode
    }
    pub fn tun_mtu(&self) -> u16 {
        self.tun.as_ref().map_or_else(default_mtu, |t| t.mtu)
    }
    pub fn dns_mode(&self) -> DnsMode {
        self.dns.as_ref().map_or(DnsMode::Tunneled, |d| d.mode)
    }
    pub fn killswitch_enabled(&self) -> bool {
        self.killswitch.as_ref().is_some_and(|k| k.enabled)
    }
    pub fn management_sources(&self) -> &[IpNet] {
        self.management
            .as_ref()
            .map_or(&[], |m| m.sources.as_slice())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Mode {
    #[default]
    Gateway,
    Proxy,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Proxy {
    #[serde(default = "default_listen")]
    pub listen: String,
    /// Required: a missing port is a parse error, not a validate error.
    pub port: u16,
}

fn default_listen() -> String {
    "::".to_string()
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Subscription {
    /// Subscription URL. The URL is itself a secret (embeds an access token);
    /// never log it verbatim — see redact::redact_url.
    pub url: String,
    /// Outbound to select. Required iff strategy = pinned; forbidden iff urltest.
    #[serde(default)]
    pub active: Option<String>,
    #[serde(default)]
    pub strategy: Strategy,
    #[serde(default)]
    pub urltest: Option<Urltest>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Strategy {
    #[default]
    Pinned,
    Urltest,
}

/// urltest knobs; defaults are the ones proven in the consumer's production HA.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Urltest {
    #[serde(default = "default_probe_url")]
    pub probe_url: String,
    #[serde(default = "default_interval")]
    pub interval: String,
    #[serde(default = "default_tolerance")]
    pub tolerance: u32,
    #[serde(default = "default_idle_timeout")]
    pub idle_timeout: String,
    #[serde(default = "default_interrupt")]
    pub interrupt_existing: bool,
}

fn default_probe_url() -> String {
    "https://www.gstatic.com/generate_204".to_string()
}
fn default_interval() -> String {
    "30s".to_string()
}
fn default_tolerance() -> u32 {
    100
}
fn default_idle_timeout() -> String {
    "30m".to_string()
}
fn default_interrupt() -> bool {
    true
}

impl Default for Urltest {
    fn default() -> Self {
        Urltest {
            probe_url: default_probe_url(),
            interval: default_interval(),
            tolerance: default_tolerance(),
            idle_timeout: default_idle_timeout(),
            interrupt_existing: default_interrupt(),
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Interfaces {
    pub wan: String,
    pub lan: String,
    pub lan_cidr: IpNet,
}

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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
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

    // Shared: subscription shape, independent of mode.
    if let Some(sub) = &cfg.subscription {
        if !(sub.url.starts_with("http://") || sub.url.starts_with("https://")) {
            errors.push(Finding::new(
                "SUBSCRIPTION_URL_INVALID",
                "subscription.url must start with http:// or https://".to_string(),
            ));
        }
        match sub.strategy {
            Strategy::Pinned => {
                if sub.active.as_deref().is_none_or(|a| a.trim().is_empty()) {
                    errors.push(Finding::new(
                        "SUBSCRIPTION_ACTIVE_EMPTY",
                        "subscription.active must name the outbound to select (strategy = pinned)"
                            .to_string(),
                    ));
                }
            }
            Strategy::Urltest => {
                if sub.active.is_some() {
                    errors.push(Finding::new(
                        "URLTEST_ACTIVE_CONFLICT",
                        "subscription.active is forbidden with strategy = urltest (the whole pool is used)".to_string(),
                    ));
                }
                if cfg.mode == Mode::Gateway {
                    errors.push(Finding::new(
                        "URLTEST_GATEWAY_UNSUPPORTED",
                        "strategy = urltest is proxy-mode-only for now".to_string(),
                    ));
                }
            }
        }
    }

    match cfg.mode {
        Mode::Proxy => validate_proxy(cfg, &mut errors),
        Mode::Gateway => validate_gateway(cfg, &mut errors, &mut warnings),
    }

    (errors, warnings)
}

fn validate_proxy(cfg: &GatewayConfig, errors: &mut Vec<Finding>) {
    match &cfg.proxy {
        None => errors.push(Finding::new(
            "PROXY_SECTION_REQUIRED",
            "mode = \"proxy\" requires a [proxy] section".to_string(),
        )),
        Some(p) if p.port == 0 => errors.push(Finding::new(
            "PORT_INVALID",
            "proxy.port must be 1..=65535".to_string(),
        )),
        Some(_) => {}
    }
    if cfg.subscription.is_none() {
        errors.push(Finding::new(
            "PROXY_SUBSCRIPTION_REQUIRED",
            "mode = \"proxy\" requires a [subscription] section".to_string(),
        ));
    }
    // Gateway sections are meaningless in proxy mode; reject them loudly rather
    // than silently ignore (the consumer's egress has no NIC/routing to speak of).
    let forbidden = [
        ("interfaces", cfg.interfaces.is_some()),
        ("routing", cfg.routing.is_some()),
        ("tun", cfg.tun.is_some()),
        ("dns", cfg.dns.is_some()),
        ("killswitch", cfg.killswitch.is_some()),
        ("management", cfg.management.is_some()),
        ("policies", !cfg.policies.is_empty()),
    ];
    for (name, present) in forbidden {
        if present {
            errors.push(Finding::new(
                "PROXY_MODE_SECTION_CONFLICT",
                format!("[{name}] is not allowed in proxy mode"),
            ));
        }
    }
}

fn validate_gateway(cfg: &GatewayConfig, errors: &mut Vec<Finding>, warnings: &mut Vec<Finding>) {
    if cfg.proxy.is_some() {
        errors.push(Finding::new(
            "GATEWAY_MODE_PROXY_SECTION",
            "[proxy] is only valid with mode = \"proxy\"".to_string(),
        ));
    }
    // interfaces/routing became Option to support proxy mode; in gateway mode
    // they are required. Guard FIRST — the accessors below would panic on None.
    if cfg.interfaces.is_none() {
        errors.push(Finding::new(
            "GATEWAY_INTERFACES_REQUIRED",
            "gateway mode requires an [interfaces] section".to_string(),
        ));
    }
    if cfg.routing.is_none() {
        errors.push(Finding::new(
            "GATEWAY_ROUTING_REQUIRED",
            "gateway mode requires a [routing] section".to_string(),
        ));
    }
    let Some(iface) = cfg.interfaces.as_ref() else {
        return;
    };
    if cfg.routing.is_none() {
        return;
    }

    if iface.wan.trim().is_empty() || iface.lan.trim().is_empty() {
        errors.push(Finding::new(
            "INTERFACE_NAME_EMPTY",
            "interfaces.wan and interfaces.lan must be non-empty".to_string(),
        ));
    }
    if iface.wan == iface.lan {
        errors.push(Finding::new(
            "WAN_LAN_SAME",
            format!(
                "interfaces.wan and interfaces.lan are both \"{}\"",
                iface.wan
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
        if !iface.lan_cidr.contains(&p.source) {
            warnings.push(Finding::new(
                "POLICY_SOURCE_OUTSIDE_LAN",
                format!(
                    "policy \"{}\" source {} is outside lan_cidr {}",
                    p.name, p.source, iface.lan_cidr
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

    if !(1280..=1500).contains(&cfg.tun_mtu()) {
        errors.push(Finding::new(
            "MTU_OUT_OF_RANGE",
            format!(
                "tun.mtu {} must be within 1280..=1500 (sing-tun drops IP fragments; >1500 blackholes PMTUD)",
                cfg.tun_mtu()
            ),
        ));
    }

    let has_vpn_traffic = cfg.routing_mode() == RoutingMode::Full
        || cfg.policies.iter().any(|p| p.route == Route::Vpn);
    if has_vpn_traffic && cfg.subscription.is_none() {
        warnings.push(Finding::new(
            "SUBSCRIPTION_MISSING",
            "vpn routing is configured but no [subscription]; the vpn outbound stays a placeholder until one is added and resolved".to_string(),
        ));
    }
    if cfg.killswitch_enabled() && !has_vpn_traffic {
        errors.push(Finding::new(
            "KILLSWITCH_WITHOUT_VPN_POLICY",
            "killswitch.enabled = true but no policy routes to vpn and routing.mode is split"
                .to_string(),
        ));
    }
    if !cfg.killswitch_enabled() && has_vpn_traffic {
        warnings.push(Finding::new(
            "KILLSWITCH_DISABLED",
            "vpn-routed traffic will leak to WAN if the tunnel goes down; consider [killswitch] enabled = true".to_string(),
        ));
    }

    if cfg.management_sources().is_empty() {
        warnings.push(Finding::new(
            "NO_MANAGEMENT_BYPASS",
            "no [management] sources configured; a bad apply can lock you out of SSH — add your admin host CIDR".to_string(),
        ));
    }
    for m in cfg.management_sources() {
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
}
