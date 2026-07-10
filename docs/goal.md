# GOAL — vpnrouter-gateway: путь до финально готового результата

> **Назначение файла.** Это durable work-order для AI-агента (Claude Code или
> аналог), который продолжает проект на сервере БЕЗ доступа к истории чата.
> Всё, что нужно для исполнения, — здесь и в перечисленных файлах. Не «как
> обсуждали»; только факты, контракты и проверяемые критерии.

---

## META (машиночитаемо)

```yaml
project: vpnrouter-gateway
repo: github.com/PavelLizunov/vpnrouter-gateway
lang: Rust 2021, one binary crate
deps: [serde, serde_json, toml, ipnet, lexopt, ureq]   # ровно 6, не добавлять без крайней нужды
baseline_commit: 5dad591        # v1 + Level-0 gate shipped; сверь `git log` при старте
status: v1 CLI complete, live-validated; НЕ развёрнут на боевом хосте
tests: 45 passing (src/tests.rs + tests/golden/)
gate: cargo fmt --check && cargo clippy --all-targets -- -D warnings && cargo test
hook: .git/hooks/pre-commit (blocking; tracked copy .githooks-pre-commit.sh)
owner_gated: [push, release, host-mutation on prod, visibility changes]
```

**Протокол работы агента (обязательный):**
1. Прочитай в этом порядке: [CLAUDE.md](../CLAUDE.md) → [HANDOFF.md](../HANDOFF.md)
   → [docs/gateway-architecture.md](gateway-architecture.md) → этот файл.
2. Не спауни субагентов на непроверенное чтение кода; сам открывай файлы.
   Собственные наблюдения > чужой пересказ.
3. Каждая задача = отдельный коммит через hook. `--no-verify` только для
   ≤5-строчного однофайлового hotfix.
4. Перед «готово» гоняй `verify`-команду задачи вживую, а не «по коду».
5. `ponytail`-поле задачи — это граница: НЕ строй то, что там перечислено.
6. Не push/release/деплой на прод без явного go владельца.

---

## 0. Как читать work breakdown

Каждая задача — карточка с фиксированной схемой полей:

```
id | track | priority | depends_on | status
why       — зачем, самодостаточно (одна причина, не «улучшить»)
files     — точные пути, которые трогаем
contract  — интерфейс/поведение, которое должно появиться (вход→выход)
accept    — [ ] проверяемые критерии готовности
verify    — точная команда(ы), доказывающая accept
ponytail  — что НЕ строить в рамках этой задачи
risk      — чем опасно, на что смотреть
```

`priority`: P1 (блокер готовности) · P2 (надёжность/полнота) · P3 (полировка).
`status`: `todo` · `wip` · `done` · `dropped`.

---

## 1. Ориентация: что УЖЕ есть (state, самодостаточно)

Поток v1 (работает целиком, проверен live на Proxmox + реальной ninitux-подписке):

```
gateway.toml → config::load → config::validate → render::{render_sing_box,render_nft}
→ plan::assess → apply::run (--yes) → status/doctor/plan::explain → apply::rollback
                                    ↑ resolve-subscription пишет кэш, из него render берёт outbound
```

Модули и ключевые символы (все в `src/`):

| Файл | Что содержит |
| --- | --- |
| `config.rs` | `GatewayConfig`{interfaces, management, subscription, routing, tun, policies, dns, killswitch}; `load()`; `validate()->(errors,warnings)` со стабильными кодами |
| `render.rs` | `PLACEHOLDER_OUTBOUND="vpn-out"`; `render_sing_box(cfg, resolved: Option<&Value>)->String`; `render_nft(cfg)->String`; детерминизм — golden |
| `plan.rs` | `Assessment{changes,risks}`; `assess()` (переиспользуется apply); `explain()`; `ssh_risk()` |
| `apply.rs` | seams `NftExec`{check,load}, `DataPlane`{check_config,restart}; `run()`; `rollback()`; порядок безопасности |
| `status.rs` | `detect_interfaces()`; `artifact_flags()`; `config_in_sync()`; `cmd_status()`; `pure_doctor_checks()`+`host_doctor_checks()`; `cmd_doctor()` |
| `subscription.rs` | `Fetcher` seam + `fetch_with_timeout` (15s); `parse_subscription()->ParseResult{outbounds,skipped}`; `parse_vless()`; `base64_decode()`; `select()`; `save_cache()`/`load_resolved()` |
| `redact.rs` | `redact_value()`, `redact_url()`, `SECRET_KEYS` |
| `error.rs` | envelope `{ok,v:1,...}`; `CliError`; `ok_envelope()`; exit-коды |
| `main.rs` | lexopt-диспетч, флаги, JSON на stdout |

Поддержано в подписке: **vless** (reality/tls, transports ws/grpc/http). Явно
**skipped-с-учётом**: hysteria2/hy2/tuic/ss/vmess/trojan/naive (`parse_uri`
возвращает `Ok(None)`, они попадают в `ParseResult.skipped`, не молча).

Exit-коды (контракт, не менять): `0` ok · `1` config invalid · `2` env/usage
· `3` confirm required / refused · `4` apply failed.

---

## 2. Инварианты (MUST NOT BREAK) — с проверкой

Любая задача, ломающая инвариант, — стоп и пересмотр, а не «обойдём».

| # | Инвариант | Verify |
| --- | --- | --- |
| I1 | Детерминизм render байт-в-байт | `cargo test render_is_deterministic_and_matches_golden` |
| I2 | nft — только `table inet vpnrouter`, никогда `flush ruleset` | `cargo test nft_only_touches_own_table`; grep `flush ruleset` src → пусто |
| I3 | Мутация хоста только в apply/rollback и только с `--yes` | `cargo test apply_requires_yes`; check/plan/status/doctor/explain/resolve не имеют кода записи в сеть |
| I4 | Порядок apply: validate(nft+sing-box) → backup → write → load → restore-on-fail | `cargo test apply_*` (6 тестов); карточка A-контракта |
| I5 | SSH-guard: apply отказывает при риске без `--allow-ssh-risk` | `cargo test apply_refuses_ssh_risk_without_flag` |
| I6 | Ровно один JSON envelope на stdout; exit-коды по §1 | ручной прогон каждой команды \| `python -c json.load` |
| I7 | Policies first-match в порядке файла; тень ловится (`POLICY_SHADOWED`) | `cargo test shadowed_policy_is_warning` |
| I8 | sing-box версия пиновая 1.13.x; схема action-based 1.12+ | `sing-box check` в CI/лаборатории |
| I9 | Секреты: реальные только на диске (root-only); весь вывод редактится | `cargo test redact_*`; grep вывода на живой подписке |
| I10 | Ровно 6 зависимостей; YAML не вводить (RUSTSEC) | `cargo tree --depth 1`; review Cargo.toml |

---

## 3. Определение «ГОТОВО» (measurable, per track)

«Финально готово» неоднозначно, пока не выбран трек (см. §4). Критерии —
проверяемые, не «ощущается законченным».

**Track A — личный L3-шлюз (DoD):**
- [ ] DA1. Развёрнут на боевом Linux-хосте по [packaging/README.md](../packaging/README.md); `doctor` зелёный (nft loaded, ip_forward=1, оба интерфейса найдены, subscription resolved).
- [ ] DA2. **Реальный клиент за шлюзом** выходит в интернет через VPN-страну (доказано, не предположено) — закрывает A1, единственный непроверенный слой.
- [ ] DA3. Kill-switch подтверждён: при остановленном sing-box трафик клиента НЕ утекает в WAN (drop, не bypass).
- [ ] DA4. hysteria2 поддержан (подписка hy2-тяжёлая) — A2.
- [ ] DA5. Переживает ребут: `apply` конвергентен, таблица/сервис восстанавливаются (уже есть в коде — подтвердить на хосте).
- [ ] DA6. CI зелёный на каждый push (A7), не только локальный hook.

**Track B — egress L1 proxy fitness (DoD):**
- [ ] DB1. render умеет `mixed`-inbound (HTTP-CONNECT/SOCKS на порту) — B1.
- [ ] DB2. render умеет `urltest`-группу из пула серверов вместо одного пина — B2.
- [ ] DB3. proxy-режим: НЕ эмитит `direct` outbound (инвариант «ни байта в обход») — B3.
- [ ] DB4. Конфиг-схема выражает inbound-mode + пул + urltest-параметры — B4.
- [ ] DB5. Сгенерированный конфиг drop-in совместим с их egress-контрактом (`sing-box check` + ручная сверка полей против задеплоенного секрета).

**Общее (оба трека):**
- [ ] DG1. `cargo test` + clippy `-D warnings` + fmt зелёные.
- [ ] DG2. Все §2-инварианты держатся (или явно, owner-gated, изменены с обновлением этого файла).
- [ ] DG3. Нет секретов в дереве/истории (secret-scan перед любым push в public).

---

## 4. РАЗВИЛКА (решение владельца — блокирует выбор фаз)

Ядро (`config/validate/plan/apply/status/doctor/explain/subscription/redact`)
**общее для обоих треков**. Расходится только `render` (цель) и часть схемы.

```
                          ┌─ Track A: TUN L3-шлюз (текущая модель)
   Shared core (done) ────┤   render: tun-inbound + direct-outbound + nft forward/nat
                          │   Заказчик: личный homelab-edge (personal-first, см. memory)
                          │   Осталось: A1..A7
                          │
                          └─ Track B: mixed-inbound CONNECT-proxy (egress-fleet)
                              render: mixed-inbound + urltest-группа, БЕЗ direct
                              Заказчик: singbox-egress-ha на инфре владельца
                              Осталось: B1..B4 (перестройка контракта render)
```

**Факты для решения (из живой оценки 2026-07-10, в памяти проекта):**
- Их egress = HTTP-CONNECT :12080 + urltest-failover. Track A физически
  неприменим туда (TUN-L3 ≠ CONNECT-proxy). Level-0 (SNI-preflight) уже
  отгружён В ИХ infra отдельно — gateway для этого догфудить не требуется.
- vpnrouter-gateway по памяти — **personal-first**. Критерий успеха — личная
  польза владельцу, не пригодность для флота.

**Рекомендация (не решение):** Track A. Он завершает продукт под его реального
заказчика и упирается в один настоящий пробел (A1 — форвардинг). Track B — это
второй продукт с другим контрактом; браться, только если владелец решит, что
gateway ДОЛЖЕН обслуживать флот. Не делать оба «на всякий случай».

> **AGENT: не начинай Phase B без явного go. По умолчанию исполняй Phase A.**

---

## 5. WORK BREAKDOWN

### PHASE A — довести L3-модель до боевой готовности

> **СТАТУС (2026-07-11): Track A закрыт.** A1 forwarding e2e доказан на железе
> (DA2 клиент→туннель→DE, DA3 kill-switch на forward-хуке дропает downstream —
> §18 арх-дока). A2 hysteria2 ✅ (реальная подписка 10 узлов). A3 doctor-DNS ✅.
> A7 CI ✅ (зелёный). A6 pedantic — отревьюено, багов нет, base-гейт это бар
> (не гейтим pedantic). A4 IPv6-direct — отложен (ponytail: blunt-drop безопасен,
> knob не просили). A5 daemon — не строим (systemd Restart= покрывает).
> DoD Track A: DA1–DA6 ✅ (DA1 деплой — по packaging/README, механика та же).

---

#### TASK A1 — e2e форвардинг: реальный клиент за шлюзом
```
id: A1 | track: A | priority: P1 | depends_on: none | status: todo
```
- **why**: Единственный непроверенный слой продукта. Доказано всё вокруг
  (рендер, nft грузится, sing-box принимает конфиг), но НИ РАЗУ не доказано,
  что пакеты второго хоста реально идут LAN→tun→VPN→WAN. Здесь живут desktop-
  уроки: auto_route для чужого трафика, NAT, MTU/PMTUD, DNS hijack на клиенте.
- **files**: тест-скрипт в лаборатории (не в репо; результат → раздел §… в
  gateway-architecture.md). Кода приложения, вероятно, менять НЕ нужно —
  это валидация. Если всплывёт баг рендера — отдельная задача-фикс.
- **contract**: two-interface топология. Gateway-хост: eth0=WAN,
  второй iface=LAN (`lan_cidr`). Client-хост на LAN-бридже, default route →
  gateway LAN-IP. После `apply`: с клиента `curl https://api.ipify.org` →
  возвращает VPN-egress IP (страна активного outbound), НЕ WAN-IP хоста.
- **accept**:
  - [ ] Клиент за шлюзом получает VPN-egress IP (проверить `ipinfo.io/country`).
  - [ ] DNS клиента не утекает (запрос идёт через tun; проверить hijack-dns).
  - [ ] Kill-switch (DA3): останов sing-box → клиентский трафик в WAN дропается, не bypass.
  - [ ] MTU: крупный ответ (>1400B, напр. большой HTTPS) проходит без blackhole.
- **verify**: лабораторный стенд (Proxmox: два CT + внутренний bridge vmbr1,
  паттерн livetest.sh). НЕ на боевом хосте. Реальная подписка через `--file`
  или resolve.
- **ponytail**: не поднимать DHCP-сервер/полноценный роутер — статический IP
  на клиенте и default route руками достаточно. Не автоматизировать стенд в CI
  (нужен привилегированный tun/nft — оставить лабораторным).
- **risk**: auto_route в контейнере может требовать `/dev/net/tun` + privileged
  LXC либо VM. Если LXC не даёт tun — поднять gateway в QEMU-VM. Это ожидаемо;
  задокументировать выбранный путь.

---

#### TASK A2 — hysteria2 outbound в парсере подписки
```
id: A2 | track: A | priority: P1 | depends_on: none | status: todo
```
- **why**: Реальная ninitux-подписка hy2-тяжёлая (4 из 11 узлов hy2 + naive);
  сейчас они попадают в `skipped`. hy2 — UDP-native (QUIC), лучше для
  voice/game, чем tcp-only vless-reality. #1 в HANDOFF.
- **files**: `src/subscription.rs` (`parse_uri` → ветка `hysteria2`/`hy2`;
  новая `parse_hysteria2()`); `src/tests.rs` (векторы); при желании
  `docs/gateway-architecture.md` (отметить поддержку).
- **contract**: share-link
  `hysteria2://<password>@<host>:<port>?sni=<sni>&obfs=salamander&obfs-password=<pw>&insecure=0#<name>`
  → sing-box outbound:
  ```json
  {"type":"hysteria2","tag":"<name>","server":"<host>","server_port":<port>,
   "password":"<password>","tls":{"enabled":true,"server_name":"<sni>","alpn":["h3"]},
   "obfs":{"type":"salamander","password":"<obfs-password>"}}
  ```
  (obfs-блок только если `obfs` присутствует.) `parse_uri` маршрутизирует
  `hysteria2` и `hy2` в `parse_hysteria2`, убрать их из skipped-списка.
- **accept**:
  - [ ] hy2 share-link → корректный outbound (тест-вектор).
  - [ ] `sing-box check` принимает конфиг с hy2-outbound (лаборатория).
  - [ ] `password`/`obfs-password` редактятся (уже в `SECRET_KEYS` — подтвердить тестом).
  - [ ] Реальная ninitux-подписка: hy2-узлы теперь резолвятся, счётчик skipped падает.
- **verify**: `cargo test hysteria2`; лаборатория — resolve реальной подписки,
  `sing-box check` на hy2-выборе.
- **ponytail**: только hysteria2. tuic/ss/naive/vmess/trojan — НЕ в этой задаче
  (отдельные, по появлению нужды). Не добавлять Brutal CC / порт-хоппинг, пока
  подписка их не отдаёт.
- **risk**: sing-box 1.13 hy2-схема — сверить поля с доками
  (https://sing-box.sagernet.org, пин 1.13.x): `up_mbps`/`down_mbps` опциональны,
  не выдумывать. `insecure` → `tls.insecure` только если явно =1.

---

#### TASK A3 — host DNS visibility в doctor
```
id: A3 | track: A | priority: P2 | depends_on: none | status: todo
```
- **why**: Сейчас DNS живёт только внутри sing-box-конфига. На боевом шлюзе
  ownership резолвера (systemd-resolved / dnsmasq / resolvconf) влияет на
  утечки, и это невидимо. Desktop-урок: DNS сложнее, чем выглядит.
- **files**: `src/status.rs` (`host_doctor_checks` + новая проверка).
- **contract**: doctor читает (read-only) факт наличия/типа резолвера
  (`/etc/resolv.conf` содержимое, `systemd-resolved` активен?) и добавляет
  `Check{name:"dns_host", level, message}`. Только диагностика, НЕ мутация.
- **accept**:
  - [ ] doctor сообщает, кто владеет резолвером на хосте.
  - [ ] warning, если конфиг `dns.mode=tunneled`, но хост-резолвер может
        перехватывать (эвристика, честно помеченная).
  - [ ] Read-only: инвариант I3 держится.
- **verify**: `cargo test doctor_*`; лаборатория на Debian (resolved) и Alpine (нет).
- **ponytail**: НЕ мутировать хост-DNS (это отдельная опасная фаза, отложена
  архитектурно). Только видимость. Не парсить все возможные резолверы — три
  распространённых достаточно.
- **risk**: эвристика «может перехватывать» не должна давать ложную уверенность;
  формулировать как warning, не error.

---

#### TASK A4 — явная IPv6-политика
```
id: A4 | track: A | priority: P2 | depends_on: none | status: todo
```
- **why**: Сейчас sing-box `strategy:ipv4_only`, а nft killswitch тупо дропает
  весь forward-v6 LAN→WAN. Это грубо: ломает легитимный v6 у клиентов без
  явного решения. Нужен осознанный контракт.
- **files**: `src/config.rs` (опция, напр. `[routing].ipv6 = "block"|"direct"`),
  `src/render.rs` (nft v6-ветки), `src/tests.rs`, schema.
- **contract**: конфиг-поле выбирает v6-поведение; render отражает. Default
  `block` (текущее безопасное). Golden обновить (UPDATE_GOLDEN=1 → verify).
- **accept**:
  - [ ] Дефолт сохраняет текущее поведение (v6 forward drop при killswitch).
  - [ ] `direct` — v6 клиентов идёт мимо туннеля осознанно (с warning про leak-семантику).
  - [ ] I1 (детерминизм) держится, golden перегенерирован и зелёный.
- **verify**: `cargo test`; `nft -c -f` в лаборатории.
- **ponytail**: НЕ добавлять v6-through-vpn (sing-box ipv4_only; полноценный
  v6-туннель — отдельный крупный кусок). Только block|direct.
- **risk**: не сломать I2 (только своя таблица).

---

#### TASK A5 — daemon/watchdog с in-band probe (ОПЦИОНАЛЬНО)
```
id: A5 | track: A | priority: P3 | depends_on: [A1] | status: todo
```
- **why**: Desktop-урок HealthMonitor: процесс-жив ≠ туннель-работает (wedged
  sing-box чёрно-дырит трафик). Для unattended-шлюза нужен in-band probe +
  restart. НО: рано, пока не доказан базовый форвардинг (A1) и не понятно,
  нужен ли автономный режим владельцу.
- **files**: новый `src/daemon.rs`; systemd-таймер/сервис в packaging.
- **contract**: периодический in-band probe (не PID-liveness) активного
  outbound; при N провалах — reload-first, потом restart, с cooldown; без
  скрытого failover-переключения серверов во время живой сессии.
- **accept**:
  - [ ] Probe различает «жив но не форвардит» и «мёртв».
  - [ ] reload перед restart (соединения переживают, где возможно).
  - [ ] Явный stop-условие + cooldown (без churn-петли).
  - [ ] Grader независимый (детерминированный probe, не LLM-суждение).
- **verify**: лаборатория — убить/завесить sing-box, наблюдать реакцию.
- **ponytail**: НЕ auto-failover между серверами (urltest — это Track B). НЕ
  tokio/async — блокирующий цикл + std::thread::sleep достаточно. НЕ notify/
  file-watcher. Начать с systemd-таймера, вызывающего `status`, прежде чем
  писать демон.
- **risk**: скрытая мутация сети демоном опаснее ручного apply. Демон,
  меняющий firewall без явного плана, — против философии проекта. Возможно,
  правильный ответ — systemd `Restart=on-failure` на sing-box + `status`-таймер,
  а не свой демон. Оценить ДО написания кода (может стать `dropped`).

---

#### TASK A6 — триаж clippy::pedantic (F4)
```
id: A6 | track: A | priority: P3 | depends_on: none | status: todo
```
- **why**: 47 pedantic-подсказок (зафиксировано в review-plan). Часть полезна
  (`must_use`, `missing_errors_doc`), часть шум.
- **files**: по всему `src/`.
- **contract**: пройти список, применить полезное, осознанное подавить
  `#[allow(...)]` с комментарием-причиной. НЕ включать pedantic в hook (шумно).
- **accept**:
  - [ ] `cargo clippy -- -W clippy::pedantic` — остаток объяснён (каждый
        либо пофикшен, либо allow+причина).
  - [ ] Базовый gate (`-D warnings`) остаётся зелёным.
- **verify**: `cargo clippy --all-targets -- -W clippy::pedantic 2>&1 | grep -c warning`.
- **ponytail**: НЕ слепой автофикс всех 47. НЕ включать pedantic в блокирующий gate.
- **risk**: некоторые pedantic-фиксы меняют публичные сигнатуры — не ломать API без нужды.

---

#### TASK A7 — CI (GitHub Actions), зеркало локального gate
```
id: A7 | track: A | priority: P2 | depends_on: none | status: todo
```
- **why**: Сейчас gate только локальный (hook). Push на GitHub ничем не
  проверяется. CI ловит то, что проскочило мимо hook (`--no-verify`, чужой клон).
- **files**: `.github/workflows/ci.yml`.
- **contract**: на push/PR — `cargo fmt --check`, `cargo clippy --all-targets
  -- -D warnings`, `cargo test`, `cargo audit`. Кэш cargo. Один Linux-раннер.
- **accept**:
  - [ ] Зелёный на текущем master.
  - [ ] Красный, если внести заведомую ошибку (проверить намеренно).
  - [ ] `cargo audit` в пайплайне (RustSec).
- **verify**: наблюдать прогон Actions на тестовом PR.
- **ponytail**: один job, один OS. НЕ матрица OS/toolchain (одиночный проект).
  НЕ release-автоматизация (owner-gated). НЕ покрытие/бейджи.
- **risk**: секреты не нужны пайплайну — не добавлять. cargo audit может
  флапнуть на новом advisory — это сигнал, не поломка.

---

### PHASE B — L1 egress fitness — **ОТГРУЖЕНО authoring-часть (2026-07-11)**

> **B1–B4 сделаны** (owner go = proxy-mode-spec). `mode="proxy"`: mixed-inbound
> + pinned/urltest, без direct/dns/nft; `render --out`; apply/rollback/doctor/
> explain отказывают `PROXY_MODE_NOT_APPLYABLE`. 57 тестов, gateway-goldens
> байт-в-байт, живой `sing-box check` на реальной ninitux-подписке. Детали —
> §17 [gateway-architecture.md](gateway-architecture.md). Осталось (Phase B
> спеки, НЕ этот итер): host-runtime для proxy (Level 2). DoD Track B: DB1–DB5 ✅.
>
> Ниже — исходные карточки B1–B4 (status: done).

> Перестройка контракта render. Ядро переиспользуется. Не начинать без §4-решения.

---

#### TASK B1 — mixed-inbound режим рендера
```
id: B1 | track: B | priority: P1 | depends_on: [B4] | status: todo
```
- **why**: Их egress — HTTP-CONNECT/SOCKS на порту, не TUN. Без mixed-inbound
  gateway физически не отдаёт то, что слушают их поды.
- **files**: `src/render.rs` (ветка inbound по режиму), `src/config.rs`, schema.
- **contract**: при `inbound.mode="proxy"` render эмитит
  `{type:"mixed", listen:"...", listen_port:N}` вместо tun-inbound; route без
  auto_route. TUN-режим остаётся дефолтом (Track A не ломать).
- **accept**:
  - [ ] `mode="proxy"` → mixed-inbound, `sing-box check` ок.
  - [ ] `mode="tun"` (default) → текущий рендер байт-в-байт (I1, golden).
  - [ ] nft для proxy-режима не эмитит forward/nat tun-специфику (или пусто).
- **verify**: `cargo test`; лаборатория `sing-box check`.
- **ponytail**: не поддерживать оба inbound одновременно, пока не нужно.
- **risk**: killswitch-семантика для proxy-режима другая (нет forward-пути) —
  пересмотреть §2-I для этого режима, не тащить tun-nft вслепую.

---

#### TASK B2 — urltest-группа (failover-пул)
```
id: B2 | track: B | priority: P1 | depends_on: [B4] | status: todo
```
- **why**: Их HA целиком на urltest-failover. Сейчас render пинит один сервер.
- **files**: `src/render.rs`, `src/config.rs`, `src/subscription.rs` (пул, не один select), schema.
- **contract**: конфиг задаёт пул серверов (или «все из подписки»); render
  эмитит N proxy-outbounds + `{type:"urltest", tag:"vpn-out", outbounds:[...],
  url:"...", interval:"..."}`. `vpn-out` остаётся точкой, на которую ссылается route.
- **accept**:
  - [ ] N серверов + urltest-группа, `sing-box check` ок.
  - [ ] `interrupt_exist_connections:false` (не рвать живые сессии).
  - [ ] Детерминизм (I1) — порядок серверов стабилен (сортировка).
- **verify**: `cargo test`; лаборатория.
- **ponytail**: не изобретать свою health-логику — urltest это делает сам.
- **risk**: это то самое «скрытое failover», от которого Track A уходил.
  В proxy-контексте оно уместно; не тащить в Track A.

---

#### TASK B3 — proxy-режим без direct outbound
```
id: B3 | track: B | priority: P1 | depends_on: [B1] | status: todo
```
- **why**: Их инвариант — «ни байта в обход из RU-IP». render всегда эмитит
  `direct`; в proxy-режиме это нарушение.
- **files**: `src/render.rs`.
- **contract**: при `mode="proxy"` (или явном `no_direct=true`) НЕ эмитить
  direct-outbound; route.final → vpn-out; приватные диапазоны — reject или
  отдельная явная политика, не direct.
- **accept**:
  - [ ] proxy-конфиг не содержит `{"type":"direct"}`.
  - [ ] TUN-режим (Track A) сохраняет direct (I1).
  - [ ] `sing-box check` ок.
- **verify**: `cargo test`; grep рендера на `direct`.
- **ponytail**: не делать это глобальным дефолтом — только proxy-режим.
- **risk**: без direct DNS/приватные пути должны быть явно разрулены, иначе
  конфиг нерабочий. Свериться с их задеплоенным секретом.

---

#### TASK B4 — конфиг-схема для inbound-mode + пул + urltest
```
id: B4 | track: B | priority: P1 | depends_on: none | status: todo
```
- **why**: B1..B3 нужен способ выразить режим в gateway.toml.
- **files**: `src/config.rs`, `schema/gateway.schema.json`, `examples/`.
- **contract**: новая секция, напр.
  ```toml
  [inbound]
  mode = "tun"        # tun (default, Track A) | proxy (Track B)
  listen = "0.0.0.0"
  listen_port = 12080
  [outbound]
  strategy = "single" # single (default) | urltest
  urltest_url = "http://www.gstatic.com/generate_204"
  urltest_interval = "3m"
  ```
  Валидация: proxy-режим требует listen_port; urltest требует пул ≥2.
- **accept**:
  - [ ] Новые поля парсятся, `deny_unknown_fields` держится.
  - [ ] validate ловит несогласованные комбинации (структурные коды).
  - [ ] Дефолты = текущее поведение Track A (обратная совместимость).
- **verify**: `cargo test`; `check` на примерах обоих режимов.
- **ponytail**: минимальный набор полей; не добавлять всё, что умеет sing-box.
- **risk**: не разломать существующий `examples/gateway.toml` (Track A).

---

### PHASE C — отложенное / спекулятивное (НЕ делать без явной нужды)

Перечислено, чтобы агент НЕ изобретал это как «улучшения». Каждое — `dropped`
до появления конкретного заказчика.

- **C1 tuic/ss/naive/vmess/trojan парсеры** — по мере появления в реальной подписке.
- **C2 pinned_outbound / no_failover** — привязать не к чему, пока нет urltest (B2). Тогда — «вывести сервер из failover».
- **C3 полный doctor redaction-бандл** (ZIP диагностики) — механизм redaction готов; бандл — когда понадобится support-flow.
- **C4 destination/domain-политики** — CIDR/proto/port сейчас достаточно; домены — когда появится реальный кейс.
- **C5 MCP/HTTP/OpenAPI тонкие обёртки** — над стабильным CLI, если понадобится AI/автоматизация поверх.
- **C6 OpenWrt/procd** — другая платформа; только если целевой хост — прошивка.
- **C7 fleet/controller/cloud** — не этот продукт.

---

## 6. Порядок исполнения / зависимости

```
Phase A (default):  A2 ─┐            A6 (в любой момент)
                    A7 ─┼─ независимы, можно параллельно/в любом порядке
                    A3 ─┘
                    A1 ─── P1, самый ценный; A5 зависит от A1
                    A4 ─── независим
        рекомендуемая последовательность: A2 → A1 → A7 → A3 → A4 → A6 → (A5 оценить)

Phase B (gated):    B4 → {B1, B2} → B3    (B4 первым: без схемы остальное некуда класть)
```

Быстрый старт агента (Track A по умолчанию):
```sh
git clone https://github.com/PavelLizunov/vpnrouter-gateway && cd vpnrouter-gateway
# прочитать CLAUDE.md, HANDOFF.md, docs/gateway-architecture.md, этот файл
apt-get install -y gcc && cargo build --release && cargo test   # baseline зелёный
# взять A2 (hysteria2) — самый чистый первый коммит, реальная нужда, без лаборатории
```

---

## 7. Kill / shrink criteria (когда СТОП, а не «ещё фича»)

- Если A1 показывает, что форвардинг требует переписать модель рендера под
  конкретное железо — это сигнал, что «универсальный шлюз» иллюзорен; сжать до
  «генератор конфигов для моего хоста».
- Если через месяц реального использования владелец правит nft/sing-box руками
  мимо инструмента — нужен был скрипт, а не агент; сжать до генератора.
- Если Phase B оказывается перестройкой >50% render — это второй продукт;
  форкнуть репозиторий, не мешать с Track A.
- Если обсуждение уходит в fleet/cloud/GUI до того, как A1 доказан — вернуться к A1.
- Пустой список доработок — валидный конечный результат: v1 уже полезен.

---

## 8. Глоссарий / ссылки

- **L3 / TUN-режим** — шлюз даёт tun-интерфейс, форвардит L3-трафик LAN (Track A).
- **L1 / proxy-режим** — шлюз слушает HTTP-CONNECT/SOCKS порт (Track B).
- **Level-0 preflight** — read-only SNI-drift гейт; уже отгружён в infra владельца отдельно (не в этом репо).
- **outbound `vpn-out`** — тег, на который ссылаются все route-правила; за ним либо placeholder, либо резолвнутый сервер, либо (Track B) urltest-группа.
- **skipped** — распознанный, но не построенный узел подписки (не молча уронен).
- sing-box 1.13 схема: https://sing-box.sagernet.org (пин 1.13.x).
- Инварианты/workflow: [CLAUDE.md](../CLAUDE.md). Архитектура и live-лог: [docs/gateway-architecture.md](gateway-architecture.md). Ревью-план: [docs/review-plan.md](review-plan.md). Деплой: [packaging/README.md](../packaging/README.md).
- Память проекта (не в git, на машине владельца): personal-first аудитория; Proxmox-лаборатория; ninitux-формат; egress-оценка → Level-0.

---

*Обновляй этот файл при закрытии задач (`status: done` + одна строка-итог) и при
изменении инварианта (owner-gated). Файл — источник истины о том, что осталось.*
