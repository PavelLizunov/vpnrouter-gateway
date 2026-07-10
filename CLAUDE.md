# vpnrouter-gateway

Headless Linux-first edge gateway: `gateway.toml -> validate -> render
(sing-box 1.13 JSON + nftables) -> plan -> apply --yes ->
status/doctor/explain -> rollback`. Плюс `mode = "proxy"` (Track B, §17):
mixed-inbound authoring-only (render --out), без direct/dns/nft, apply отказывает.
Rust, один бинарь, шесть зависимостей
(serde, serde_json, toml, ipnet, lexopt, ureq). Вся архитектура, вердикты и
live-валидация: [docs/gateway-architecture.md](docs/gateway-architecture.md).

## Команды

- `cargo test` — весь suite, включая golden-тесты
  (`UPDATE_GOLDEN=1 cargo test` регенерирует `tests/golden/*`, затем обычный
  прогон должен быть зелёным).
- `cargo clippy --all-targets -- -D warnings` и `cargo fmt --check` — обязаны
  быть чистыми.
- Linux-бинарь: собирать **нативно на Linux** (`cargo build --release`, нужен
  gcc — ureq тянет ring/TLS). Кросс-musl с Windows требует
  `x86_64-linux-musl-gcc`; нативная сборка — boring-путь. См. packaging/README.md.
- Живая лаборатория: Proxmox LXC (доступ и IP — в памяти проекта;
  `livetest.sh` — 8-шаговый сценарий apply/rollback в контейнере).

## Инварианты (не ломать)

- Детерминизм render: байт-в-байт, закреплено golden-тестами.
- Добавление режимов не должно сдвигать gateway-goldens (proxy — отдельная
  `render_proxy_sing_box`, не шарит код). `UPDATE_GOLDEN` на gateway-golden
  обязан давать нулевой дифф.
- nftables: только собственная таблица `inet vpnrouter`; `flush ruleset`
  запрещён навсегда.
- Мутации хоста только в `apply`/`rollback` и только с `--yes`; остальные
  команды read-only по построению.
- Порядок apply: `nft -c` кандидата ДО любых изменений → backup current в
  last-good ДО замены → провал загрузки восстанавливает и перезагружает
  прежнее. Apply конвергентен (перезагружает таблицу даже без файловых
  изменений — чинит состояние после ребута).
- SSH-guard: apply отказывается при риске SSH_MAY_DROP без явного
  `--allow-ssh-risk`.
- Вывод: ровно один JSON envelope на stdout (`{"ok":..,"v":1,...}`).
  Exit-коды: 0 ok, 1 config, 2 env/usage, 3 confirm/refused, 4 apply failed.
- Policies — first-match-wins в порядке файла; специфичные раньше широких
  (validate ловит тень POLICY_SHADOWED).
- sing-box версия пиновая: 1.13.x, схема 1.12+ action-based.

## Workflow

- Pre-commit hook (`.git/hooks/pre-commit`) блокирует commit:
  `fmt --check` + `clippy -D warnings` + `cargo test`. `--no-verify` — только
  для ≤5-строчного однофайлового hotfix.
- Не push и не release без явного go владельца.
- Секретов в конфиге пока нет; redaction обязателен с появлением
  `[subscription]` (URL подписки = секрет).
