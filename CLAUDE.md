# vpnrouter-gateway

Headless Linux-first edge gateway: `gateway.toml -> validate -> render
(sing-box 1.13 JSON + nftables) -> plan -> apply --yes ->
status/doctor/explain -> rollback`. Rust, один бинарь, пять зависимостей
(serde, serde_json, toml, ipnet, lexopt). Вся архитектура, вердикты и
live-валидация: [docs/gateway-architecture.md](docs/gateway-architecture.md).

## Команды

- `cargo test` — весь suite, включая golden-тесты
  (`UPDATE_GOLDEN=1 cargo test` регенерирует `tests/golden/*`, затем обычный
  прогон должен быть зелёным).
- `cargo clippy --all-targets -- -D warnings` и `cargo fmt --check` — обязаны
  быть чистыми.
- Linux-бинарь с Windows: `cargo build --release --target
  x86_64-unknown-linux-musl` (rust-lld, см. `.cargo/config.toml`) — статический,
  работает на glibc и musl.
- Живая лаборатория: Proxmox LXC (доступ и IP — в памяти проекта;
  `livetest.sh` — 8-шаговый сценарий apply/rollback в контейнере).

## Инварианты (не ломать)

- Детерминизм render: байт-в-байт, закреплено golden-тестами.
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
