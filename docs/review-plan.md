# Review plan: полный аудит кода + сверка с доками библиотек + best practices

Дата составления: 2026-07-09. Исполнять на сервере (нативный Linux) или
review-агентом. Формат находок — в конце (§8). Код: 9 модулей `src/*.rs`,
6 зависимостей, инварианты в [CLAUDE.md](../CLAUDE.md), архитектура в
[gateway-architecture.md](gateway-architecture.md).

Цель ревью — не «нравится/не нравится», а: (1) корректность против доков
sing-box 1.13 / nftables / крейтов; (2) поведение на недоверенном вводе
(подписка = сеть); (3) соблюдение собственных инвариантов проекта;
(4) покрытие тестами реальных рисков.

## 0. Как запускать (детерминированные грейдеры прежде глаз)

```sh
cargo fmt --check
cargo clippy --all-targets -- -D warnings           # обязано быть чисто (есть)
cargo clippy --all-targets -- -W clippy::pedantic    # 47 подсказок — триаж, не слепой фикс
cargo test                                           # 44 теста
cargo install cargo-audit cargo-deny                 # если ещё нет
cargo audit                                          # RustSec advisories (сейчас 0)
cargo deny check licenses bans sources               # лицензии/дубликаты/источники
```

Ревью-агента брифовать как нового коллегу: приложить diff/файлы + инварианты
из CLAUDE.md, не «как обсуждали». Верификатор — независимый (не самооценка).

## 1. Подтверждённые находки из pre-review прогона (сиды, проверять первыми)

| # | Приоритет | Файл | Находка |
| --- | --- | --- | --- |
| F1 | **P1 корректность** | [subscription.rs](../src/subscription.rs) `RealFetcher::get` | Фетч подписки **без таймаута** — зависший сервер блокирует `resolve-subscription` навсегда. Фикс: `ureq::Agent` с `timeout_global`/`timeout_connect` (~15s), как было заявлено в доке. Проверить API ureq 3.3 (Agent config builder). |
| F2 | P3 стиль | [subscription.rs:255](../src/subscription.rs) | `ob["tag"].as_str().unwrap()` — безопасен (tag выставлен строкой выше), но reviewer флагнёт: заменить на локальную `name` без индексации/unwrap. |
| F3 | инфо | Cargo/serde_json | Детерминизм render **подтверждён**: serde_json без `preserve_order` → `Value` = сортированный BTreeMap; `indexmap` в графе сборки отсутствует; закреплено golden-тестом. Не regression-риск, но задокументировать зависимость инварианта от фичи. |
| F4 | P3 | весь src | 47 `clippy::pedantic` подсказок. Не фиксить слепо; триажить (напр. `must_use`, `#[allow]` на осознанных местах). |

## 2. Пофайловый deep-review

### config.rs — модель + валидация (trust boundary: TOML пользователя)
- Все структуры под `#[serde(deny_unknown_fields)]`? (да — подтвердить, что новые тоже).
- Полнота `validate`: есть ли конфиги, которые проходят validate, но дают
  сломанный render? Проверить: `route="block"` + `port` (порт игнорится?);
  `lan_cidr` как host `/32`; дублирующиеся `management.sources`; политика с
  `source` шире `lan_cidr` (warning есть); MTU-границы 1280..=1500.
- `SUBSCRIPTION_URL_INVALID` — только префикс-чек `http(s)://`; достаточно ли
  (не парсим host)? Для MVP да, отметить как осознанное.
- Тень политик (`POLICY_SHADOWED`) ловит только точный superset по
  source+proto+port. Частичные пересечения (напр. разные порты одной подсети)
  — не тень, но проверить, что матчинг first-match в render это отражает.

### render.rs — sing-box JSON + nft (главный инвариант: детерминизм + своя таблица)
- **Сверка каждого поля sing-box против доков 1.13** (§4).
- nftables-синтаксис: `meta nfproto ipv6 drop`, `meta l4proto`, `udp dport`,
  family-префиксы `ip`/`ip6` по IpNet. Живьём принято ядром 1.0.6, но
  проверить семантику: killswitch на forward-hook покрывает LAN→WAN; direct
  accept выше drop; management accept первым.
- `vpn_outbound` ретег в `vpn-out`: убедиться, что клонируется и что все
  route-правила и dns-detour ссылаются ровно на `vpn-out`.
- IPv6: при killswitch дропаем forward v6 LAN→WAN; но direct/management v6 —
  что с ними? sing-box конфиг `ipv4_only`. Проверить, нет ли v6-утечки мимо
  туннеля для не-vpn трафика.

### plan.rs — assess (переиспользуется apply), explain, ssh_risk
- `assess` — единственный источник changes/risks и для plan, и для apply
  (инвариант «что в plan — то и гейтит apply»). Подтвердить.
- `ssh_risk`: только TCP (UDP-политики пропускаются), first-match, management
  освобождает. Проверить парсинг `SSH_CONNECTION` (первое поле = client IP),
  IPv6-клиент.
- `explain`: детерминированный matcher, обязан зеркалить порядок render
  (management → policies first-match → routing.mode). Сверить ветку за веткой
  с render.rs; расхождение = баг доверия.

### apply.rs — единственная мутация (порядок безопасности — контракт)
- Порядок: check nft + check sing-box (оба кандидата) ДО изменений → backup →
  write → nft load → best-effort restart. Разобрать **частичные сбои**:
  - `write_artifact` sing-box ок, nft rename падает → состояние?
  - backup скопировал 1 из 2 файлов и упал?
  - nft load падает БЕЗ backup (первый apply) → удаляем новые артефакты, ядро
    не тронуто — проверить, что sing-box.json тоже удаляется.
- Rollback-на-load-failure перезагружает прежние правила — проверить путь,
  когда и восстановление падает (сообщение есть, состояние — задокументировать).
- Гейт sing-box: отсутствующий бинарь = reported skip (не гейт). Осознанно;
  отметить риск (плохой конфиг пройдёт, если sing-box не в PATH при apply).
- TOCTOU на `state_dir` (между assess-чтением current и записью) — оценить.
- best-effort restart: `systemctl restart` роняет туннель ~16s (урок desktop).
  Проверить, что это отражено в выводе и не делает health-loop.

### subscription.rs — парсер недоверенного сетевого ввода (не паниковать!)
- **F1 (таймаут) — фикс.**
- base64-декодер: свой, tolerant. Фаззить/векторы: пустой, невалидный
  алфавит, паддинг посреди, url-safe vs standard, не-кратная длина.
- `percent_decode`: `%` в конце строки, невалидный hex (`%ZZ`), `%` без двух
  символов — не паниковать (проверено `i+2 < len`, подтвердить).
- `parse_vless`: отсутствующие поля (нет `@`, нет порта, нет query, нет
  fragment), IPv6-host `[::1]:443`, порт вне u16, security=none (без tls-блока).
- JSON-wrapper: recursion depth guard = 3. Проверить циклический/глубокий
  враппер не зациклит и не переполнит стек.
- Учёт unsupported: hy2/tuic/ss/naive считаются, при 0 поддержанных — ошибка со
  списком. Проверить сообщение и что реальная ninitux-подписка (6 vless из 11)
  резолвится (проверено live).
- `ureq` `read_to_string`: есть ли лимит размера тела по умолчанию в ureq 3?
  Большая подписка не должна молча обрезаться — **сверить с доками ureq**.

### redact.rs — маскирование секретов для вывода
- `SECRET_KEYS` покрывает: uuid, password, private_key, pre_shared_key, psk,
  obfs_password, auth, token, secret, short_id. Сверить с полями будущих
  протоколов (hy2: `password`/`obfs`; trojan/ss: `password`). `public_key`
  (reality pbk) — публичен, сохраняется. Ок.
- Дыра desktop (числовое значение под неизвестным ключом проходит): здесь
  редактируем только резолвнутые outbounds известной формы, но при расширении
  на произвольный JSON — вернуть проверку. Задокументировать границу.
- `redact_url`: userinfo (`user:pass@`) срезается, path/query отброшены,
  битый URL → `***`. Проверить edge: `scheme://` без host; `scheme://@host`.

### error.rs / main.rs
- Ровно один JSON envelope на stdout, exit-коды 0/1/2/3/4 консистентны.
- Парсинг аргументов (lexopt): `--proto` не tcp/udp, `--port` вне u16,
  `--source` не IP → структурная usage-ошибка (проверено). Дубли флагов
  (последний выигрывает?) — поведение задокументировать.
- `resolve-subscription` без `--active`/config: листинг available (не пишет
  кэш). Подтвердить, что кэш не трогается при листинге.

## 3. Инварианты проекта — отдельный gate (из CLAUDE.md)
Каждый проверить тестом или ручным аудитом:
- [ ] Детерминизм render байт-в-байт (golden). ✅ F3.
- [ ] nft: только `table inet vpnrouter`, `flush ruleset` не встречается нигде.
- [ ] Мутации только в apply/rollback и только с `--yes`; check/plan/status/
      doctor/explain/resolve физически без записи в сеть.
- [ ] Порядок apply (nft -c → backup → load → restore).
- [ ] SSH-guard блокирует без `--allow-ssh-risk`.
- [ ] Один JSON envelope; exit-коды по спеке.
- [ ] Policies first-match в порядке файла; тень ловится.
- [ ] sing-box пиновая 1.13.x.

## 4. Сверка с документацией библиотек

| Крейт | Версия | Что сверить с актуальными доками |
| --- | --- | --- |
| serde | 1.0.228 | `deny_unknown_fields` + `#[serde(default)]` на Option/Vec; `rename_all="lowercase"` для enum. |
| serde_json | 1.0.150 | `Value` = BTreeMap без `preserve_order` (F3). `to_string_pretty`. `json!` не паникует на нашей форме. |
| toml | 1.1.2 | API `from_str`; поведение `deny_unknown_fields` с toml-таблицами; нет ли нужды в `toml = "0.9"` по MSRV. |
| ipnet | 2.12.0 | **`IpNet::contains(&IpNet)`** = superset (на этом стоит shadow-детект и management/policy матчинг) — сверить сигнатуру и семантику. `contains(&IpAddr)`. |
| lexopt | 0.3.2 | `Parser::from_env`, `Long`, `Value`, `parser.value()`; обработка `--flag=value` vs `--flag value`. |
| ureq | 3.3.0 | **API 3.x**: `get().call().body_mut().read_to_string()` (используется). **Таймауты** (F1) — Agent config builder. **Лимит размера тела** read_to_string. TLS: rustls default + корни (webpki-roots?) — проверить, что HTTPS-валидация включена (проверено live против google). |

MSRV: зафиксировать (edition 2021). `cargo msrv` при желании.

## 5. Best practices / безопасность
- [ ] Нет `unwrap/expect/panic` на недоверенном вводе (F2 — единственный
      unwrap, безопасный). Грепнуть повторно после изменений.
- [ ] Все IO → структурный `CliError`, не `?` в `String`.
- [ ] Trust boundary «подписка»: парсер не паникует ни на каком байте (фаззинг
      `cargo fuzz` или proptest — опционально).
- [ ] Секреты: реальные — только на диске root-only; вывод редактится; URL =
      секрет. Проверить, что логов с секретами нет (logging пока нет).
- [ ] Права: apply/rollback требуют root по факту (nft); документировано.
      systemd unit — `CAP_NET_ADMIN`, не полный root.
- [ ] `.gitattributes` LF — golden побайтово; sh-скрипты. Есть.
- [ ] CI отсутствует (осознанно, одиночный проект) — при желании GitHub Actions
      прогоняющий тот же gate; hook уже принудительный локально.

## 6. Доменная корректность
- **sing-box 1.13 schema**: tun (`address`, `auto_route`, `strict_route:false`,
  `stack:"system"`, `route_exclude_address`, `mtu`), route action-based
  (`sniff`/`hijack-dns`/`ip_is_private`/`source_ip_cidr`/`action:reject`),
  dns (`type:https`, `detour`, `default_domain_resolver`, `strategy:ipv4_only`),
  vless outbound (reality/utls/flow/packet_encoding, transport ws/grpc/http).
  Сверить каждое поле с https://sing-box.sagernet.org (пиновать 1.13.x доки).
- **nftables**: killswitch на forward — покрытие всех leak-путей LAN→WAN;
  NAT masquerade для direct/management; v6-политика.
- **DNS-утечки**: tunneled → vpn-dns detour vpn-out; локальные приватные —
  direct. Проанализировать, не утекает ли DNS при падении туннеля (killswitch
  дропает, но DNS-запрос?).
- **MTU**: 1420 default, clamp 1280..=1500 — урок sing-tun (дроп фрагментов).

## 7. Пробелы в тестах (кандидаты дописать)
- Частичный сбой apply (write/backup/rename падает на 2-м файле) — восстановление.
- `percent_decode` edge (`%` в конце, `%ZZ`).
- vless IPv6-host, отсутствующие поля, security=none.
- IPv6-политика в render (сейчас только v4 в примерах).
- base64 фаззинг/векторы граничные.
- explain ⇔ render зеркальность (property: для случайного source вердикт
  explain == outbound, который выбрал бы render).

## 8. Формат находок
На каждую находку: `file:line` — one-line суть — конкретный failure-сценарий
(вход → неверный выход/паника) — verdict CONFIRMED/PLAUSIBLE — предлагаемый
фикс (минимальный). Ранжировать P1 корректность/безопасность → P2 надёжность →
P3 стиль. Пустой список — валидный результат, если ничего не выжило проверку.
Верификация — независимая (deterministic gate прежде LLM-суждения).

---
Быстрый старт ревью: `§0` прогнать → `§1` подтвердить сиды (начать с F1) →
`§2` пофайлово → `§4` сверка доков → `§6` домен → `§7` дописать тесты.
