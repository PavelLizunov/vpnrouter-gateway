# AGENTS.md — vpnrouter-gateway

Onboarding for a coding agent (Codex or other) continuing this project without
the prior chat. Everything needed to work safely is here or in the linked docs.
Not "as we discussed" — only facts, contracts, and runnable checks.

## What this is

Headless Linux-first edge gateway in Rust. One config `gateway.toml` → validated
desired state → deterministic render → plan → apply/rollback, with a strict JSON
CLI. Two modes share one core:

- **`mode = "gateway"` (default):** renders a sing-box 1.13 **TUN** config +
  **nftables** (own table only), and mutates the host (`apply --yes`,
  `rollback --yes`). This is a real L3 edge: LAN clients route through the VPN.
- **`mode = "proxy"`:** renders a sing-box **`mixed` inbound** (HTTP-CONNECT/
  SOCKS) with a pinned outbound or a `urltest` failover group. **Authoring-only:**
  no `direct`/`dns`/`route.rules`/nft; `render --out DIR` writes the artifact for
  the consumer's own deploy pipeline; `apply`/`rollback`/`doctor`/`explain`
  refuse (`PROXY_MODE_NOT_APPLYABLE`, exit 2).

One binary crate, **exactly 6 dependencies** (serde, serde_json, toml, ipnet,
lexopt, ureq). Subscription protocols parsed: **vless** (Reality/TLS, ws/grpc/http)
and **hysteria2** (QUIC), plus sing-box JSON passthrough; others surfaced as
`skipped` with a reason.

**Source of truth:** [docs/gateway-architecture.md](docs/gateway-architecture.md)
(full design + live-validation log; §17 proxy mode, §18 forwarding proof).
Roadmap/what's-left: [docs/goal.md](docs/goal.md). Review checklist:
[docs/review-plan.md](docs/review-plan.md). Deploy: [packaging/README.md](packaging/README.md).

## Build / test / gate

```sh
# Native Linux build (needs a C compiler: ureq's TLS pulls `ring`).
apt-get install -y gcc      # or: apk add gcc musl-dev
cargo build --release

# The gate — ALL must be clean before any commit:
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test
cargo audit                 # RustSec; keep 0 (CI runs it)
```

- **Do NOT cross-compile from Windows/macOS** — `ring` needs a target C toolchain
  (`x86_64-linux-musl-gcc`); native Linux build is the boring path.
- **Golden regen:** `UPDATE_GOLDEN=1 cargo test`, then a plain `cargo test` must be
  green. The two gateway goldens (`tests/golden/sing-box.json`, `nft.rules`) must
  show **zero git diff** after regen — adding modes/knobs must never shift them.
- Tests live in `src/tests.rs` (unit) + `tests/golden/*` (byte-compared) +
  `tests/fixtures/*`. 62 tests currently.

## Hard invariants (MUST NOT break)

| # | Invariant | Check |
|---|---|---|
| I1 | Render is byte-identical for identical inputs (determinism) | `cargo test render_is_deterministic_and_matches_golden`, `proxy_render_matches_golden` |
| I2 | nftables touches **only** `table inet vpnrouter`; `flush ruleset` never appears | `cargo test nft_only_touches_own_table`; `grep -rn "flush ruleset" src/` → only the guard test |
| I3 | Host mutation only in `apply`/`rollback`, only with `--yes`; check/plan/status/doctor/explain/render/resolve are read-only | `cargo test apply_requires_yes` |
| I4 | apply order: validate nft (`nft -c`) **and** sing-box (`check`) → backup → write → load → restore-on-fail; convergent | `cargo test apply_*` (8 tests) |
| I5 | Exactly one JSON envelope on stdout `{"ok":..,"v":1,...}`; exit codes `0` ok / `1` config / `2` env-usage / `3` confirm-refused / `4` apply-failed. **A panic breaks this** — the `.expect()` accessors (`interfaces()`, `routing_mode()`, `routing_ipv6()`) must stay behind a mode-fork or a prior `validate()`/`is_none()` guard | run any command, `python -c 'json.load(...)'` |
| I6 | SSH-guard: gateway `apply` refuses `SSH_MAY_DROP` without `--allow-ssh-risk` | `cargo test apply_refuses_ssh_risk_without_flag` |
| I7 | Secrets (uuid/url/password/obfs) redacted in ALL stdout; real secrets only on disk (0600) | `cargo test redact_*`, `render_command_envelope_is_content_free` |
| I8 | Policies first-match in file order; shadow caught (`POLICY_SHADOWED`) | `cargo test shadowed_policy_is_warning` |
| I9 | sing-box pinned 1.13.x (action-based 1.12+ schema) | lab `sing-box check` |
| I10 | Exactly 6 dependencies; **no YAML** (RustSec) | `cargo tree --depth 1` |

Determinism note: `serde_json` runs **without** `preserve_order`, so `Value` is a
sorted `BTreeMap` → stable key order. Do not enable `preserve_order`.

## Module map (`src/`)

| File | Role |
|---|---|
| `config.rs` | `GatewayConfig` + all types; `mode`, `proxy`, six gateway sections are `Option` (presence-detectable); accessors (`tun_mtu()`, `dns_mode()`, `interfaces()`, `routing_mode()`, `routing_ipv6()`); `load()`; `validate()` forked by mode with stable error codes |
| `render.rs` | `render_sing_box(cfg, resolved)` (gateway TUN), `render_nft(cfg)` (own table), `render_proxy_sing_box(cfg, outbounds)` (mixed inbound; `dedup_tags` collision-proof). **Two separate render fns — do not merge** (gateway `log:info` vs proxy `log:warn`, no dns/route in proxy) |
| `subscription.rs` | `Fetcher` seam + `fetch_with_timeout` (15s); `parse_subscription` (JSON-wrapper/base64/URI-list; vless + hysteria2); node-safety filter (plaintext/reality-without-pbk → `Skipped{reason}`); cache **v2** (all outbounds) + v1 compat + loud-on-unknown-version; `save_cache*`/`load_cache`/`load_resolved` (0600) |
| `plan.rs` | `assess` **forks by mode** before any gateway field access; `explain` (deterministic matcher); `ssh_risk`; `proxy_outbounds` (single source of truth for pinned vs urltest) |
| `apply.rs` | `apply::run`/`rollback`; `NftExec` + `DataPlane` seams (tests inject fakes); safety-ordered mutation |
| `status.rs` | `cmd_status`/`cmd_doctor` (read-only host probes, degrade to "unknown"); `config_in_sync` forks by mode + **guards `interfaces` AND `routing`** (cmd_status doesn't validate); `dns_host_check` |
| `redact.rs` | `redact_value` (secret-key masking, recurses), `redact_url` (host-only) |
| `error.rs` | `CliError`, `ok_envelope`, exit codes, `v:1` |
| `main.rs` | lexopt dispatch, `refuse_if_proxy`, `write_artifact` (0600, path+bytes only), one JSON envelope + exit |

## Workflow (enforced — read before committing)

- **Blocking pre-commit hook** (`.git/hooks/pre-commit`; tracked copy
  `.githooks-pre-commit.sh`) runs fmt + clippy `-D warnings` + test. `--no-verify`
  is allowed only for a true ≤5-line single-file hotfix.
- **Do NOT push or release without the owner's explicit go.** Commit locally; ask.
- **The GitHub repo is PUBLIC.** Never commit secrets: subscription URLs/tokens,
  the owner's lab credentials, SSH keys, real uuids. `examples/*.toml` use
  placeholder URLs (`example.com`). Run a secret-scan of the diff before any push.
- CI (`.github/workflows/ci.yml`) mirrors the gate + `cargo audit` on push/PR.
- One task = one clean commit through the hook. Don't `git add -A` untracked
  files you didn't create into your commit.

## Verification tiers

- **Deterministic (always available to you):** `cargo test`, clippy, golden
  byte-parity, `cargo audit`, and manual reachability analysis of `.expect()`
  call sites. These are the primary graders — more reliable than LLM review here.
- **Lab (owner's private Proxmox; credentials are out-of-band, NOT in this repo):**
  real `sing-box 1.13.14 check` on rendered artifacts, and the forwarding e2e.
  The forwarding test needs a **privileged LXC with `/dev/net/tun` passthrough**
  (unprivileged LXC has no tun) + an internal bridge + a client CT. If you cannot
  reach the lab, rely on deterministic graders and ask the owner to run lab checks
  for any change to the emitted network shapes.

## Current state (2026-07-11)

Both products shipped and validated on real hardware:

- **Gateway (Track A):** complete. Forwarding e2e proven in the lab — a LAN client
  behind the gateway egresses through the VPN (DE), and the **forward-hook
  kill-switch drops downstream traffic** when the tunnel dies (the exact gap the
  reference desktop app had). hysteria2 lands the real subscription's 10 nodes.
  CI green. See §18 of the architecture doc.
- **Proxy (Track B, authoring):** complete. Real `sing-box check` accepts pinned
  and urltest renders (vless + hy2, domain servers, no dns block). See §17.

**Deferred — do NOT build without the owner's explicit go:**

- Proxy **Level-2 host-runtime** (systemd unit serving the proxy on a box; nft-less
  apply/doctor). The consumer spec explicitly says "NOT this spec, do not build now".
- **daemon/watchdog** — systemd `Restart=on-failure` covers it; a daemon that
  mutates network state is against the project's plan-before-apply philosophy.
- **tuic/ss/naive/vmess/trojan** URI parsing — add when a real subscription needs
  them (currently correctly `skipped` with a reason).
- OpenWrt, HTTP/MCP wrappers, fleet/controller — speculative; out of scope.

**DA1 (deploy on the owner's real box)** is the owner's action; the mechanics are
lab-proven and documented in `packaging/README.md`.

## Gotchas / lessons (non-obvious)

- Adding a mode/knob must keep gateway goldens byte-identical — proxy has its own
  `render_proxy_sing_box`, and new `Option` sections default to old behavior via
  accessors. Always `UPDATE_GOLDEN` and confirm zero diff on the two gateway goldens.
- All six gateway sections (`interfaces`/`routing`/`tun`/`dns`/`killswitch`/
  `management`) are `Option` so proxy mode can *reject* their presence; `serde`
  can't distinguish an omitted `[dns]` from a defaulted one otherwise.
- `cmd_status` does NOT call `validate()` — any render path it reaches
  (`config_in_sync`) must guard missing `interfaces`/`routing` or the `.expect()`
  accessors panic (a real bug fixed on 2026-07-11).
- `dedup_tags` must be collision-proof: input `[A, A, A-1]` → `[A, A-1, A-1-1]`,
  never a duplicate tag (route.final / urltest.outbounds reference these).
- hysteria2 share links carry a trailing `/` in `host:port` (`host:8444/`) — strip
  the path before parsing. Real panels also set `insecure=1` + salamander obfs.
- Windows/git-bash caveat: git-bash paths (`/c/...`) differ from what native
  Windows `python`/tools read (`C:/...`); prefer the Rust tools + `cargo` which
  handle this.

## How to continue

1. Read: this file → [docs/gateway-architecture.md](docs/gateway-architecture.md)
   → [docs/goal.md](docs/goal.md).
2. Make the change; keep the invariants; add/adjust a test.
3. Run the full gate; for any change to the emitted sing-box/nft shape, get a lab
   `sing-box check` (ask the owner if you lack lab access).
4. Commit through the hook (one clean commit). Do not push/release without the
   owner's go.
