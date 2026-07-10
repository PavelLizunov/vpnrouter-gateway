# Handoff

Headless Linux VPN gateway (Rust + sing-box + nftables). Full design,
decisions, and live-validation log: [docs/gateway-architecture.md](docs/gateway-architecture.md).
Invariants and workflow: [CLAUDE.md](CLAUDE.md). Deploy: [packaging/README.md](packaging/README.md).

## Status (2026-07-11)

v1 gateway CLI + **proxy mode (Track B authoring)** complete. Gateway:
`gateway.toml → validate → render (sing-box 1.13 + nftables) → plan →
apply --yes → status/doctor/explain → rollback → resolve-subscription`. Proxy:
`mode="proxy"` → mixed inbound + pinned/urltest, no direct/dns/nft, `render
--out` (apply/rollback/doctor refuse). 57 tests, clippy `-D warnings` + fmt
clean, blocking pre-commit hook, gateway goldens byte-identical. Live-validated
on Proxmox LXC (Debian 12 / Alpine 3.23): gateway apply/rollback/kill-switch;
real ninitux subscription resolves; real sing-box 1.13.14 accepts both gateway
AND proxy (urltest, 6 vless, domain servers) renders; gateway connectivity
egresses DE; bad reality short_id → apply refuses (can't brick the box). See
[docs/gateway-architecture.md](docs/gateway-architecture.md) §17 for proxy mode.

## Build on the target server (native Linux — the boring path)

```sh
apt-get install -y gcc            # ureq's TLS pulls `ring`, needs a C compiler
cargo build --release             # or: cargo test
install -m0755 target/release/vpnrouter-gateway /usr/local/bin/
```

Needs the official `sing-box` 1.13.x at `/usr/local/bin/sing-box`
(Alpine/musl also needs `apk add gcompat`). Then follow packaging/README.md.

## Secret hygiene

The repo contains NO secrets (scanned tree + full history: no tokens, real
server IPs, or credentials; `examples/gateway.toml` is a placeholder). The real
subscription URL is a secret (embeds a token) — put it ONLY in
`/etc/vpnrouter/gateway.toml` on the box (root, `0600`). It is redacted in all
tool output; resolved outbounds (uuid etc.) live only under `/var/lib/vpnrouter`
(root-only).

## What does NOT travel with this repo

- **Claude memory** — lives in `~/.claude/…` on the original Windows machine,
  not in git. If continuing with Claude Code on the server, that context is
  gone; this file + the docs are the durable handoff.
- **The Proxmox test lab** — LAN-only (`192.168.x`), unreachable from an
  external server. Test on the target box directly, or stand up a fresh local
  env. Lab details are in the machine-local memory, not here.

## Recommended next work

1. **hysteria2 outbound** — the real subscription is hy2-heavy (4 of 11 nodes)
   and hy2 is UDP-native (QUIC), better for voice/game than TCP-only
   vless-reality. Highest-value addition for the actual use case.
2. Client-behind-gateway **forwarding test** (LAN client → tun → WAN) — the one
   thing not yet proven; needs a two-interface topology.
3. Deferred items: tuic/ss/naive parsing, `pinned_outbound`/failover, full
   doctor redaction bundle, daemon/watchdog — see docs §16.
