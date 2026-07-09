# VPNRouter Gateway: deep MVP review

Дата проверки: 2026-07-06.

## 1. Executive verdict

Делать только как spike, не как сразу "corporate router edition".

Смысл есть, но он уже: не "замена роутера" и не "корпоративный VPN-продукт",
а маленький Linux gateway agent, который делает одну сильную вещь: превращает
явный `gateway.toml` в проверяемый sing-box/nftables state с `plan`, безопасным
`apply`, rollback и хорошей диагностикой.

Reason-to-build появляется там, где desktop VPN уже не подходит:

- нужен один предсказуемый edge для офиса/мини-ПК/VM;
- маршрутизация должна быть объяснима, а не зависеть от GUI-click state;
- real-time UDP нельзя тихо гонять через random failover/urltest;
- админ должен видеть план изменений до того, как firewall/routes меняются.

Если после spike не получится показать "я понимаю, что будет применено, могу
не отрезать себе SSH, могу откатиться", проект лучше остановить. Без этого это
будет просто еще один wrapper над sing-box.

## 2. Самые важные инсайты

1. Главная фича не VPN, а доверие к изменениям сети. `plan`, `doctor`,
   rollback, SSH-lockout warning и redacted diagnostics важнее, чем еще один
   routing knob.

2. "Corporate" в названии опасно. Реальный первый рынок - homelab, small office,
   dev team gateway, офисный edge. Настоящий enterprise быстро потребует fleet,
   RBAC, audit log, HA, support contracts. Это не MVP.

3. Desktop-проект уже доказал, что lifecycle и авто-магия дороги. В gateway надо
   меньше automatic behavior, больше explicit state.

4. `daemon` не нужен в первом MVP. CLI с `check`, `plan`, `apply --yes`,
   `status`, `doctor`, `rollback` проще, тестируемее и безопаснее.

5. Subscription fetch не должен быть скрытой частью `apply`. Иначе один сетевой
   timeout меняет смысл плана. В spike можно не fetch-ить вообще; в v1 лучше
   иметь `resolve-subscription` или cache/pin активного outbound.

6. nftables надо использовать через свою таблицу/цепочки. Не надо flush ruleset.
   Gateway должен быть хорошим соседом на Linux-хосте.

7. AI-friendly полезно только как строгий CLI/JSON/schema. MCP позже как тонкий
   adapter. Если строить "AI API" раньше локального агента, это упаковка без
   продукта.

## 3. Что показал осмотр `C:\Project\VPNRouter`

Проект осмотрен read-only. Изменений в `C:\Project\VPNRouter` не вносилось.

Что можно переиспользовать концептуально:

- Модель генерации sing-box config. Desktop уже генерирует TUN inbound,
  outbounds, route rules, DNS и учитывает порядок правил:
  `C:\Project\VPNRouter\VPNRouter.Core\Services\ConfigGenerator.cs:127`,
  `:149`, `:153`, `:162`.
- Накопленные MTU/DNS уроки. В коде есть явные фиксы вокруг jumbo MTU,
  UDP-native WireGuard/AWG и game UDP:
  `ConfigGenerator.cs:31`, `:49`, `:1125`.
- Subscription parsing как знание о грязном реальном мире: JSON wrapper,
  raw base64, plain URI, Clash YAML, placeholder filtering:
  `SubscriptionFetcher.cs:34`, `:126`, `:207`, `:234`.
- Diagnostics redaction: fail-safe allowlist, unknown fields redacted,
  structured JSON/YAML redaction, bounded log tails:
  `DiagnosticsRedactor.cs:13`, `:17`, `:292`,
  `DiagnosticsExporter.cs:7`, `:72`.
- Linux nftables lesson: per-process kill-switch на Linux не получается честно
  сделать через nftables по process image. Desktop Linux kill-switch признан
  full-tunnel-only/global:
  `LinuxFirewallManager.cs:15`, `:90`, `:95`.

Что нельзя тащить:

- Avalonia UI, ViewModels, tray/lifecycle, self-update, desktop autostart.
- Per-process routing как главный primitive. Gateway должен мыслить source
  CIDR, destination, proto/port, DNS policy, management bypass.
- YAML config и layered migrations из desktop. Gateway лучше начать с одного
  TOML и жесткой schema.
- Multi-platform abstraction soup. Gateway runtime Linux-first; portable только
  config/render/plan.
- Auto-failover как скрытое поведение. Для UDP/voice/game sessions это как раз
  источник недоверия.

Какие риски desktop-подхода подтверждаются:

- Текущий `CURRENT_STATE.md` прямо говорит, что Linux shipped firewall manager
  еще no-op/no DNS hardening для leak backstop:
  `C:\Project\VPNRouter\CURRENT_STATE.md:27`.
- README подтверждает desktop nature: per-process split tunnel, GUI/session
  autostart, no systemd service:
  `README.md:81`, `:96`, `:232`.
- UDP degradation detector есть, но runtime not wired:
  `UdpDegradationDetector.cs:19`. Значит проблему нельзя считать решенной
  текущим desktop engine.

## 4. Что в архитектуре правильно

- Отдельный продукт. Desktop app не надо превращать в gateway.
- Desired-state flow: `gateway.toml -> validate -> DesiredState -> render ->
  plan -> apply -> status/doctor/rollback`.
- Linux-first, systemd-first. OpenWrt позже.
- sing-box как data plane, nftables/routes/DNS как host integration.
- CLI/JSON first. Это проще тестировать и автоматизировать, чем HTTP server.
- TOML вместо YAML. Меньше сюрпризов и меньше supply-chain риска.
- Shell-out first. `nft -c -f`, `nft -f`, `ip -j`, `resolvectl`, `systemctl`
  дают достаточно для MVP.
- Rollback generated artifacts, а не "магический undo всего Linux".

## 5. Что спорно или опасно

- `apply` может отрезать SSH. Нужен explicit management bypass и warning по
  текущему SSH source. Без этого проект нельзя ставить на remote gateway.
- "Replace nft ruleset" опасно. Надо управлять только своей таблицей, например
  `inet vpnrouter`, и не трогать чужие firewall rules.
- DNS сложнее, чем выглядит. `systemd-resolved`, NetworkManager, resolvconf,
  dnsmasq и OpenWrt имеют разные ownership models. В MVP DNS лучше рендерить и
  диагностировать, а mutate делать отдельной фазой.
- Rollback routes/DNS/firewall не равен rollback файлов. Нужно last-good files
  плюс команды восстановления. В spike достаточно доказать file rollback model.
- `explain` может стать псевдо-ИИ. В MVP это должен быть детерминированный
  matcher по DesiredState: source/dest/proto/port -> выбранная policy -> route.
- `daemon` рано. Автоматический reconcile, который меняет сеть, до зрелого
  `plan/apply` вреден.
- `capabilities --json` полезно, но не должно отвлекать от реальных сетевых
  инвариантов.

## 6. Что выкинуть из MVP

- GUI.
- HTTP server, OpenAPI, MCP.
- Fleet/controller/cloud.
- OpenWrt support.
- nftables/netlink Rust crates.
- Runtime daemon/reconcile mutating loop.
- Domain sets и remote rule-set management.
- Автоматический subscription refresh в `apply`.
- Auto-failover во время active UDP sessions.
- `notify` file watcher.
- Layered config через `config`/`figment`.
- JSON Schema generator dependency, пока schema маленькая.

## 7. Что обязательно оставить

- `gateway.toml`.
- Strict validation.
- Deterministic render of `sing-box.json` and `nft.rules`.
- `plan` before any mutation.
- `apply --yes` only.
- Own nftables table/chain.
- `rollback`.
- `status` and `doctor`.
- Redaction for support bundle.
- SSH/admin lockout warning.
- Management bypass policy.
- JSON envelopes and stable error codes.

## 8. Обновленный MVP scope

Spike 0, no real apply:

- `schema --json`
- `check --config ... --json`
- `plan --config ... --json`
- `detect-interfaces --json`

V1 local agent:

- `apply --config ... --yes --json`
- `status --json`
- `doctor --json`
- `rollback --yes --json`
- systemd unit for running sing-box, not for hidden config mutation.

V1 should support only:

- one WAN interface;
- one LAN interface/subnet;
- source CIDR policies;
- route: `vpn`, `direct`, `block`;
- optional proto/port for UDP pinning;
- explicit management bypass;
- DNS mode: `tunneled` or `direct`, rendered/validated first.

## 9. Минимальный module layout

```text
src/
  main.rs       # lexopt, command dispatch, JSON stdout
  config.rs     # GatewayConfig, TOML load
  validate.rs   # pure validation + warnings
  desired.rs    # normalized DesiredState
  render.rs     # sing-box JSON + nft rules text
  plan.rs       # diff current files vs rendered files
  apply.rs      # backup/write/shell-out, cfg linux
  status.rs     # ip -j, nft -j, systemctl queries
  doctor.rs     # bundle inputs + redaction
  error.rs      # ErrorEnvelope and codes
```

One binary crate. No workspace split until it hurts.

## 10. CLI contract v1

Spike:

```text
vpnrouter-gateway schema --json
vpnrouter-gateway check --config /etc/vpnrouter/gateway.toml --json
vpnrouter-gateway plan --config /etc/vpnrouter/gateway.toml --json
vpnrouter-gateway detect-interfaces --json
```

V1:

```text
vpnrouter-gateway capabilities --json
vpnrouter-gateway apply --config /etc/vpnrouter/gateway.toml --yes --json
vpnrouter-gateway status --json
vpnrouter-gateway doctor --json
vpnrouter-gateway explain --config /etc/vpnrouter/gateway.toml --source 192.168.10.50 --dest 1.1.1.1 --proto udp --port 443 --json
vpnrouter-gateway rollback --yes --json
```

Later:

```text
vpnrouter-gateway daemon
```

Read-only:

- `schema`
- `capabilities`
- `detect-interfaces`
- `check`
- `plan`
- `status`
- `doctor`
- `explain`

Mutating/root-only:

- `apply --yes`
- `rollback --yes`
- later `daemon` if it mutates state

## 11. JSON schemas/envelopes v1

Success:

```json
{
  "ok": true,
  "data": {}
}
```

Error:

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
  "config_path": "/etc/vpnrouter/gateway.toml",
  "changes": [
    {
      "target": "sing-box",
      "action": "replace_file",
      "path": "/var/lib/vpnrouter/current/sing-box.json"
    },
    {
      "target": "nftables",
      "action": "replace_own_table",
      "path": "/var/lib/vpnrouter/current/nft.rules"
    }
  ],
  "risks": [
    {
      "level": "warning",
      "code": "NO_MANAGEMENT_BYPASS",
      "message": "No direct management bypass policy is configured"
    }
  ]
}
```

Status:

```json
{
  "ok": true,
  "data": {
    "applied": false,
    "sing_box": { "running": false, "pid": null },
    "nftables": { "table_present": false },
    "interfaces": [
      { "name": "eth0", "state": "up", "addresses": ["192.0.2.10/24"] }
    ]
  }
}
```

Doctor:

```json
{
  "ok": true,
  "checks": [
    { "name": "config", "level": "ok", "message": "Config parses and validates" },
    { "name": "nft", "level": "warning", "message": "nft not found" }
  ],
  "bundle_path": null
}
```

## 12. `gateway.toml` v1

```toml
[interfaces]
wan = "eth0"
lan = "br0"
lan_cidr = "192.168.10.0/24"

[subscription]
url = "https://example.com/sub"
active = "Germany VLESS"

[routing]
mode = "full" # full | split

[[policies]]
name = "management-bypass"
source = "192.168.10.50/32"
route = "direct"

[[policies]]
name = "office-default"
source = "192.168.10.0/24"
route = "vpn"

[[policies]]
name = "voice-udp-pinned"
source = "192.168.10.0/24"
protocol = "udp"
port = 50000
route = "vpn"
pinned_outbound = "Germany VLESS"
no_failover = true

[dns]
mode = "tunneled" # tunneled | direct

[killswitch]
enabled = true
```

Validation:

- `wan != lan`.
- `lan_cidr` parses as CIDR.
- policy names unique.
- at least one policy.
- `route` in `vpn | direct | block`.
- `routing.mode` in `full | split`.
- `dns.mode` in `tunneled | direct`.
- `port` requires `protocol`.
- `pinned_outbound` requires `route = "vpn"`.
- `no_failover` requires `pinned_outbound`.
- `killswitch.enabled` requires at least one `vpn` policy.
- warning if no policy name contains `management` or no direct policy for a
  host CIDR.

Do not add destination domain sets in spike. CIDR/proto/port is enough to
prove the architecture.

## 13. First spike plan

Goal: prove the product can produce a deterministic, inspectable network plan
without touching the host network.

Steps:

1. Create Rust binary crate in `D:\vibe-code\vpn-rust`.
2. Add only:

   ```toml
   serde = { version = "1", features = ["derive"] }
   serde_json = "1"
   toml = "1"
   ipnet = { version = "2", features = ["serde"] }
   lexopt = "0.3"
   ```

   Skip `ureq`, `log`, `systemd-journal-logger` until they are used.

3. Implement `check`.
4. Implement render to memory:
   - minimal `sing-box.json`;
   - minimal `nft.rules` for own table.
5. Implement `plan` diff against absent/current files.
6. Implement static `schema --json`.
7. Add sample config and one `cargo test`.

Success criteria:

- Invalid interface/policy config returns structured error.
- Valid sample emits stable plan JSON.
- Rendered nft rules include only `table inet vpnrouter`.
- No command mutates network state.
- Test proves parse -> validate -> render -> plan.

Test:

```text
cargo test
```

Assertions:

- sample config loads two or three policies;
- management bypass is detected;
- nft render contains LAN CIDR and vpnrouter table;
- plan contains sing-box and nftables changes when current files do not exist.

## 14. Kill criteria

Stop or radically shrink if:

- `plan` cannot clearly predict what `apply` will change.
- SSH/admin lockout cannot be detected well enough for remote use.
- nftables integration requires taking over the whole host firewall.
- sing-box config for gateway mode becomes mostly copied desktop complexity.
- real users only want "install sing-box config on router", not a managed agent.
- first testers do not use `doctor`/`plan` outputs to debug real issues.
- OpenWrt becomes mandatory before Linux MVP proves itself.
- product discussion keeps drifting to GUI/fleet/cloud before local agent works.

## 15. Next steps

1 день:

- Build spike 0: `schema`, `check`, `plan`.
- Add sample `gateway.toml`.
- Add one cargo test.

1 неделя:

- Add `detect-interfaces`.
- Add `status` read-only via `ip -j`, `nft -j`, `systemctl`.
- Add `nft -c -f` validation behind plan.
- Run on a disposable Linux VM.

1 месяц:

- Add guarded `apply --yes`.
- Add last-good backup and `rollback --yes`.
- Add redacted doctor bundle.
- Test SSH lockout warnings on a remote VM.
- Decide whether this is a product or just a useful internal appliance tool.

## Crate check

Latest stable via crates.io API on 2026-07-06:

| crate | latest | decision |
| --- | --- | --- |
| serde | 1.0.228 | keep |
| serde_json | 1.0.150 | keep |
| toml | 1.1.2+spec-1.1.0 | use `toml = "1"` |
| ipnet | 2.12.0 | keep |
| ureq | 3.3.0 | later, when subscription fetch exists |
| lexopt | 0.3.2 | keep |
| log | 0.4.33 | later, when logging exists |
| systemd-journal-logger | 2.2.2 | later, Linux-only |
| schemars | 1.2.1 | skip for now |
| clap | 4.6.1 | skip for small CLI |
| tokio | 1.52.3 | skip |
| reqwest | 0.13.4 | skip |
| notify | 8.2.0 stable / 9.0.0-rc.4 newest | skip |
| config | 0.15.25 | skip |
| figment | 0.10.19 | skip |
| serde_yaml | 0.9.34+deprecated | skip |
| serde_yml | 0.0.13 | skip |
| yaml-rust | 0.4.5 | skip |

OSV version-specific query showed no advisories for the proposed latest MVP set
(`serde`, `serde_json`, `toml`, `ipnet`, `ureq`, `lexopt`, `log`,
`systemd-journal-logger`). YAML remains a bad default: `serde_yml 0.0.13`
matches `RUSTSEC-2025-0068`; `yaml-rust 0.4.5` matches `RUSTSEC-2024-0320`.

## External sources checked

- crates.io API: `https://crates.io/api/v1/crates/<crate>`
- OSV API: `https://api.osv.dev/v1/query`
- RustSec `serde_yml`: https://rustsec.org/advisories/RUSTSEC-2025-0068.html
- RustSec `yaml-rust`: https://rustsec.org/advisories/RUSTSEC-2024-0320.html
- sing-box route actions: https://sing-box.sagernet.org/configuration/route/rule_action/
- nftables JSON/ruleset behavior: https://wiki.nftables.org/wiki-nftables/index.php/Output_text_modifiers
- systemd service units: https://www.freedesktop.org/software/systemd/man/systemd.service.html
- MCP tools schema concept: https://modelcontextprotocol.io/specification/2025-06-18/server/tools

