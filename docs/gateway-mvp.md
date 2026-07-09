# VPNRouter Gateway MVP

Date checked: 2026-07-06.

## Verdict

The architecture is sound: do not turn the Avalonia desktop app into a router.
Make a separate Linux-first, headless gateway agent with one config file,
deterministic render output, `plan` before `apply`, and rollback.

Bad ideas to avoid in MVP:

- HTTP server, web UI, MCP server, controller/fleet plane.
- Async stack just to fetch subscriptions.
- nftables/netlink Rust bindings before shell-out becomes painful.
- Hidden failover for latency-sensitive UDP sessions.
- Daemon reconcile that mutates firewall state before `plan/apply` is boring and trusted.

## Crates

Current choices still make sense. Update only what buys something now.

```toml
[dependencies]
serde = { version = "1", features = ["derive"] }
serde_json = "1"
toml = "1"
ipnet = { version = "2", features = ["serde"] }
ureq = "3"
lexopt = "0.3"
log = "0.4"

[target.'cfg(target_os = "linux")'.dependencies]
systemd-journal-logger = "2"
```

Checked latest stable versions via crates.io API:

| crate | latest |
| --- | --- |
| serde | 1.0.228 |
| serde_json | 1.0.150 |
| toml | 1.1.2+spec-1.1.0 |
| ipnet | 2.12.0 |
| ureq | 3.3.0 |
| lexopt | 0.3.2 |
| log | 0.4.33 |
| systemd-journal-logger | 2.2.2 |

Notes:

- `toml = "1"` is now the boring default. Use `0.9` only if MSRV or distro packaging forces it.
- `ureq = "3"` is fine for blocking subscription/probe fetches.
- `lexopt` is still the smallest reasonable parser for this CLI.
- Keep `systemd-journal-logger` Linux-only.
- Do not add `schemars` yet. Return a checked-in/static JSON Schema first; generate it later if it drifts.
- Keep YAML out. `serde_yml` has RustSec unsound/unmaintained history, and `yaml-rust` is unmaintained.

## MVP Architecture

```text
gateway.toml
  -> config::load
  -> validate::check
  -> desired::from_config
  -> render::{sing_box,nft}
  -> plan::diff_files
  -> apply::{backup_current,write_current,shell_out}
  -> status/doctor/rollback
```

MVP applies generated files, not clever object-level mutations.

Filesystem:

```text
/etc/vpnrouter/gateway.toml
/var/lib/vpnrouter/current/sing-box.json
/var/lib/vpnrouter/current/nft.rules
/var/lib/vpnrouter/last-good/sing-box.json
/var/lib/vpnrouter/last-good/nft.rules
/var/lib/vpnrouter/plans/last-plan.json
```

Shell-out boundary:

- `nft -c -f <file>` for validation.
- `nft -f <file>` only during confirmed apply.
- `ip -j addr`, `ip -j route` for detect/status.
- `systemctl reload-or-restart sing-box` only after config write succeeds.

## Module Layout

```text
src/
  main.rs          # lexopt CLI, JSON stdout/stderr
  config.rs        # GatewayConfig + TOML load
  validate.rs      # pure validation
  desired.rs       # normalized DesiredState
  render.rs        # render sing-box JSON and nft rules
  plan.rs          # Plan, Change, Risk
  apply.rs         # backup/write/shell-out, Linux-only
  status.rs        # detect interfaces, routes, service state
  doctor.rs        # diagnostics + redaction
  error.rs         # structured machine errors
```

No workspace split until this hurts. One binary crate is enough.

## CLI Contract

Read-only:

```text
vpnrouter-gateway schema --json
vpnrouter-gateway capabilities --json
vpnrouter-gateway detect-interfaces --json
vpnrouter-gateway check --config /etc/vpnrouter/gateway.toml --json
vpnrouter-gateway plan --config /etc/vpnrouter/gateway.toml --json
vpnrouter-gateway status --json
vpnrouter-gateway doctor --json
vpnrouter-gateway explain --config /etc/vpnrouter/gateway.toml --source 192.168.10.50 --dest 1.1.1.1 --proto udp --port 443 --json
```

Mutating:

```text
vpnrouter-gateway apply --config /etc/vpnrouter/gateway.toml --yes --json
vpnrouter-gateway rollback --yes --json
vpnrouter-gateway daemon
```

Rules:

- `apply` without `--yes` returns a structured error and does nothing.
- `apply` always runs the same code path as `plan` first.
- `apply` writes last-good before replacing current generated artifacts.
- `daemon` in MVP can be a status/reconcile loop, but no automatic mutating reconcile yet.

## JSON Shapes

Success envelope:

```json
{
  "ok": true,
  "data": {}
}
```

Error envelope:

```json
{
  "ok": false,
  "code": "WAN_INTERFACE_NOT_FOUND",
  "message": "Interface eth0 was not found",
  "suggestions": [
    {
      "command": "vpnrouter-gateway detect-interfaces --json",
      "reason": "List available interfaces"
    }
  ],
  "safe_to_retry": true
}
```

Plan:

```json
{
  "ok": true,
  "changes": [
    {
      "target": "sing-box",
      "action": "replace_config",
      "path": "/var/lib/vpnrouter/current/sing-box.json"
    },
    {
      "target": "nftables",
      "action": "replace_ruleset",
      "path": "/var/lib/vpnrouter/current/nft.rules"
    }
  ],
  "risks": [
    {
      "level": "warning",
      "code": "SSH_MAY_DROP",
      "message": "Current SSH client may be affected by this policy"
    }
  ]
}
```

Capabilities:

```json
{
  "ok": true,
  "commands": ["schema", "capabilities", "detect-interfaces", "check", "plan", "apply", "status", "doctor", "explain", "rollback", "daemon"],
  "config_formats": ["toml"],
  "data_plane": ["sing-box", "nftables", "linux-routes"],
  "apply_requires": ["root", "--yes"]
}
```

## gateway.toml Schema

Minimal config:

```toml
[interfaces]
wan = "eth0"
lan = "br0"

[subscription]
url = "https://example.com/sub"
active = "Germany VLESS"

[routing]
mode = "full" # full | split

[[policies]]
name = "office-default"
source = "192.168.10.0/24"
route = "vpn" # vpn | direct | block

[[policies]]
name = "admin-direct"
source = "192.168.10.50/32"
route = "direct"

[dns]
mode = "tunneled" # tunneled | direct

[killswitch]
enabled = true
```

Rust shape:

```rust
struct GatewayConfig {
    interfaces: Interfaces,
    subscription: Option<Subscription>,
    routing: Routing,
    policies: Vec<Policy>,
    dns: Dns,
    killswitch: Killswitch,
}

struct Policy {
    name: String,
    source: ipnet::IpNet,
    destination: Option<ipnet::IpNet>,
    protocol: Option<Protocol>,
    port: Option<u16>,
    route: Route,
    pinned_outbound: Option<String>,
    no_failover: Option<bool>,
}
```

Validation:

- `interfaces.wan != interfaces.lan`.
- At least one policy.
- Policy names unique.
- `routing.mode` is `full` or `split`.
- `route` is `vpn`, `direct`, or `block`.
- `port` requires `protocol`.
- `pinned_outbound` requires `route = "vpn"`.
- Killswitch requires at least one `vpn` policy.
- Warn if no explicit management/admin bypass exists.

## First Spike

Build only this:

1. `check`: parse TOML, validate, emit JSON.
2. `plan`: render `sing-box.json` and `nft.rules` to memory/temp files, compare with current files, emit changes/risks.
3. `schema`: return static JSON Schema.
4. `detect-interfaces`: shell out to `ip -j addr` on Linux; return unsupported on non-Linux.

Do not implement real apply in the first spike. If implemented anyway, hide it behind `--yes`, run `nft -c` first, backup current, then write files.

Smallest useful self-check:

- One sample `gateway.toml`.
- One `cargo test` that parses it and asserts:
  - two policies are loaded;
  - rendered nft rules contain the LAN CIDR;
  - plan has a sing-box change and nftables change when current files are absent.

## Later, Not MVP

- MCP adapter over CLI.
- HTTP/OpenAPI.
- OpenWrt `procd`.
- nftables/netlink crate.
- Controller/fleet management.
- GUI.

