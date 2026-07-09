# VPNRouter Gateway: архитектурный вердикт и план spike

Дата: 2026-07-09. Статус: консолидирует и местами **оспаривает**
`gateway-deep-review.md` (2026-07-06) и `gateway-mvp.md`.
Источники: read-only осмотр `C:\Project\VPNRouter` (двумя независимыми
проходами), crates.io + OSV на 2026-07-09, оба существующих документа.

---

## 1. Executive verdict

**Делать только spike. Не делать "corporate router edition".**

Прежний вердикт из deep-review подтверждаю, но осмотр кода desktop-проекта
добавил два факта, которые *усиливают* аргумент за spike и *ослабляют*
аргумент за большой продукт:

1. Desktop-проект подтверждает кодом, что вся его сложность сидит в двух
   вещах, которых в gateway **нет**: per-process routing (`process_name`,
   macOS helper-expansion, ETW rescan) и desktop lifecycle/privilege
   (sudo -n, wintun warm-up, modern standby). Gateway-версия этих проблем
   не наследует — значит, честный gateway может быть *радикально* меньше,
   чем экстраполяция от desktop подсказывает.

2. Оба существующих gateway-документа пропустили три вещи, без которых
   продукт как router не работает вообще: **FORWARD hook** (desktop
   kill-switch блокирует только `output` — на роутере это значит, что
   трафик клиентов LAN вообще не защищён), **NAT/masquerade** (direct-трафик
   клиентов должен NAT-иться на WAN — ни в одном документе нет nat chain)
   и **fail-closed семантику** (desktop сознательно fail-open — для
   gateway это неправильный default). Это не повод не делать — это повод
   не доверять "архитектура уже готова, осталось написать".

Если spike не докажет за ~1 день кода, что `gateway.toml -> plan` даёт
детерминированный, объяснимый, безопасный план — проект остановить.

## 2. Product sense

**Пользователь (уточнено 2026-07-09): в первую очередь сам владелец
проекта.** Gateway — personal-first инструмент для его собственной сети;
рыночный продукт — опциональное «потом». Это меняет критерии успеха:
не «есть ли рынок», а «экономит ли это владельцу время и нервы против
ручного sing-box + nftables». Соответственно, все product-market
активности (naming, onboarding, тестеры) из скоупа выпадают, а скоуп
фич режется под реальную сеть и подписку владельца.

Обобщённый портрет (на случай будущей продуктизации): админ homelab /
small office, у которого уже есть sing-box-подписка (VLESS/Reality,
Hysteria2, TUIC) и обычный Linux-хост (мини-ПК, VM, NUC), и который хочет
пустить через неё *сегмент сети*, а не одно устройство — предсказуемо и
без страха отрезать себе SSH.

**Боль:** сегодня это делается руками — sing-box config + nftables + ip rule
+ resolv.conf, всё разъезжается, откат — это "вспомнить, что менял".
Ни plan, ни rollback, ни диагностики.

**Отличия от альтернатив — честно:**

| Альтернатива | Чем gateway отличается | Где альтернатива побеждает |
| --- | --- | --- |
| Desktop VPN (наш же) | Вся LAN, headless, без GUI-state | Одно устройство, per-app routing |
| sing-box вручную | plan/apply/rollback, nft+DNS+NAT оркестрация, doctor | Полная гибкость, ноль новых инструментов |
| OpenWrt + homeproxy/passwall | Работает на любом systemd-Linux, JSON CLI, plan-before-apply | Уже есть GUI и комьюнити; если у юзера роутер с OpenWrt — он не придёт |
| Tailscale/Headscale | Другая задача: у них mesh/identity, у нас "LAN через коммерческую подписку с policy" | Всё, что про связность устройств, а не про egress-политику |
| OPNsense/pfSense/коммерческие | sing-box-протоколы (Reality/Hy2/TUIC) в форм-факторе роутера | Зрелый firewall, GUI, HA, поддержка |

**Strongest reason-to-build:** комбинация "censorship-resistant протоколы
sing-box" + "ops-дисциплина plan/apply/rollback/doctor" + "обычный Linux,
не прошивка" не существует как продукт. Homeproxy даёт первое без второго
и требует OpenWrt; OPNsense даёт второе без первого.

**Почему это не просто wrapper над sing-box:** sing-box решает туннель.
Он не решает: forward-политику хоста, kill-switch для чужого трафика, NAT,
DNS ownership на хосте, SSH-lockout, rollback, диагностику. Ценность —
именно в host integration + доверии к изменениям. Если убрать plan/rollback
/doctor — остаётся wrapper, и его делать не надо (kill-критерий №1).

**Где проект может оказаться бессмысленным:** если реальная аудитория —
это люди, которым достаточно "скопируй мой sing-box config на роутер"
(один раз, руками), а не управляемый агент. Это проверяется только
первыми тестерами: смотрят ли они в `plan`/`doctor` при отладке. Не
смотрят — продукт не нужен, есть appliance-скрипт.

**"Corporate" из названия убрать.** Enterprise немедленно требует fleet,
RBAC, audit, HA — это не MVP и, вероятно, вообще не этот продукт.

## 3. Находки в C:\Project\VPNRouter

Осмотр read-only, изменений не вносилось. Desktop v2.46.0, data plane
sing-box 1.13.x (Windows 1.13.14, Linux CI 1.13.10), схема конфига
action-based 1.12+/1.13.

### Уроки, которые берём (концептуально, не кодом)

- **sing-box 1.13 схема как контракт.** TUN inbound `stack:"system"`,
  route rules action-based, `strategy:"ipv4_only"` против v6-утечек,
  `urltest` группы, валидация конфига через `sing-box check`. Версию
  data plane **пинить явно** — desktop это выучил дорого (AWG/XHTTP
  требуют форк sing-box-lx; официальный 1.13.14 их отвергает).
- **MTU — фундаментальное ограничение, не деталь.** sing-tun system stack
  **молча дропает IP-фрагменты без PMTUD-сигнала** (`stack_system.go:571`).
  MTU > 1500 = blackhole для DoH/HTTP2 (Roblox Error 277, зафиксировано
  в `ConfigGenerator.cs:20-49`); MTU < ~1330 = дроп игрового DF-UDP
  (Dota/CS2 SDR шлют до 1328 без PMTUD). Gateway наследует это полностью:
  clamp ≤1500, default 1420, и это должно быть валидируемое поле, а не магия.
- **DoH поверх UDP-native туннеля дедлочится** на MTU (TLS ServerHello
  flight); внутри уже зашифрованного туннеля DNS надо гонять plain UDP.
- **QUIC поверх TCP-only прокси = head-of-line meltdown**; митигируется
  reject QUIC → HTTP/2 fallback, только когда нет UDP-capable outbound.
- **Валидировать каждый referenced outbound tag** до применения. Класс
  бага "route rule ссылается на не-emitted outbound → весь трафик тихо
  падает в direct" (v2.28.2) — теперь hard throw. Тот же урок: malformed
  Reality `short_id` **паникует sing-box при загрузке конфига** — крипто-поля
  из подписки валидировать до render.
- **In-band serving probe, не PID-liveness.** Живой процесс ≠ работающий
  туннель: wedged sing-box (процесс жив, Clash API молчит) blackhole-ит
  всё. Detector: N тиков "alive но не отвечает /version" → kill → recover
  (`HealthMonitor.cs:419-440`). Ровно это должен делать v1 `status`/watchdog.
- **Hot-reload прежде restart.** Reload через API сохраняет соединения;
  full restart роняет TUN на ~16с. Двухступенчатое восстановление +
  cooldown — правильная форма для v1 apply.
- **Kill-switch как проекция состояния, fail-closed на crash, отложенное
  снятие блока** до подтверждения "туннель реально отвечает" — blueprint в
  `LinuxFirewallManager.BuildRuleset` годный: своя таблица, atomic `-f` file,
  политика drop, marker-file для orphan cleanup.
- **Diagnostics redaction — самая зрелая подсистема, брать почти как есть:**
  allowlist для structured data (ключи сохраняются, значения → `***`),
  unknown string fields redacted by default, fail-closed на parse failure
  (весь файл → omitted), URL → scheme://host без path/query/userinfo,
  `SecretKeys` обходит numeric fast-path (числовой `short_id` не утечёт).
  Покрыто сильными тестами, включая E2E "спрятанный секрет не выживает в ZIP".
- **Subscription parsing знает грязный мир:** base64 / JSON-wrapper /
  Clash YAML / plain URI, never-throw, keep-cached-on-empty-fetch,
  placeholder-фильтр, scrub секретов *до* записи в лог.
- **Форма UDP-degradation детектора** (fire только на fully-dead: таймауты ≥ N
  И ноль успехов; cooldown 10 мин) — здравая. Но: **детектор не подключён
  в runtime** (`UdpDegradationDetector.cs:19-23`), пороги не тюнились на
  живом репро. Брать форму, не числа, и не обещать auto-detection в v1.

### Что нельзя тащить

- Per-process routing как primitive — на gateway нет локальных процессов;
  мыслить source CIDR / iface / proto / port.
- Avalonia, lifecycle, self-update, autostart, tray.
- `sudo -n` NOPASSWD модель — daemon получает `CAP_NET_ADMIN` через
  systemd `AmbientCapabilities`, sudo не нужен.
- **Fail-open kill-switch.** Desktop сознательно fail-open ("не брикнуть
  ноутбук"). Для gateway это неверный threat model — см. §8.
- Marker-file "уберём при следующем запуске GUI" — у daemon "следующий
  запуск" может не наступить; teardown через systemd `ExecStopPost`.
- YAML, layered config migrations, multi-platform abstraction.
- Статические god-классы с mutable static seams — desktop сам это
  документирует как tech debt.

### Подтверждённые кодом проблемы, критичные для gateway

1. **Kill-switch desktop-Linux висит только на `output` hook** — путь
   `forward` не покрыт вообще. На десктопе это норм; на роутере значит
   "клиенты LAN утекают в WAN при падении VPN". Оба gateway-дока это
   пропустили.
2. Шипнутый Linux desktop — **no-op firewall manager, без DNS hardening**
   (`CURRENT_STATE.md:26-33`). Linux-десктоп не является gateway-базой.
3. nftables **не умеет match по process image** — per-process kill-switch
   на Linux невозможен честно; desktop это признал no-op'ом
   (`LinuxFirewallManager.cs:88-98`).
4. Детерминизм вывода ConfigGenerator **нигде не тестируется** (grep по
   тестам пуст). Gateway должен пинить byte-for-byte детерминизм тестом
   с первого дня — на нём стоит вся модель plan/diff.
5. One-shot IPv4-only DNS snapshot IP серверов при arm — при ротации DNS
   апстрима блок-окно превращается в брик. Gateway: пере-резолв как часть
   apply, и явная обработка IPv6.
6. Redaction-дыра: **число под неизвестным ключом проходит нетронутым**
   (numeric fast-path). В gateway-редакторе числа сохранять только под
   allowlisted-ключами.

## 4. MVP scope

**Spike 0 (без мутаций хоста):** `schema`, `check`, `plan`,
`detect-interfaces`. Никакого apply, daemon, fetch, HTTP, async.
Ответы на прямые вопросы: daemon в MVP **не нужен**; real apply в первом
spike **не нужен**; subscription fetch в spike **не нужен** (см. §7);
`pinned_outbound`/`no_failover` в spike **не нужны**; destination
CIDR/domain в spike **не нужны**.

**V1 (локальный агент):** guarded `apply --yes` (nft -c → backup → write →
nft -f → reload sing-box), `rollback --yes`, `status` (включая in-band
probe sing-box), `doctor` (с redaction), `explain` (детерминированный
matcher), `capabilities`, `resolve-subscription` (fetch отдельной командой,
кэш + pin — не внутри apply). systemd unit для sing-box.
Один WAN, один LAN, source-CIDR policies, route ∈ vpn|direct|block,
proto/port, management bypass, DNS render+validate (mutation хоста DNS —
отдельная фаза даже в v1: сначала рендерим и проверяем, мутируем позже).

**Later (не обещать):** daemon/reconcile, MCP/HTTP/OpenAPI как thin
wrappers, OpenWrt, fleet, destination domain sets, runtime UDP-degradation
detection, netlink/nftables crates, GUI.

**Минимальный прототип с максимумом знания** — именно spike 0: он
проверяет единственную гипотезу продукта ("явный конфиг → проверяемый
план, которому веришь") без единого риска для хоста, работает даже на
Windows dev-машине (всё, кроме detect-interfaces, — чистые функции),
и его артефакты (`sing-box.json`, `nft.rules`) можно руками проверить
`sing-box check` и `nft -c -f` на любой VM ещё до написания apply.

## 5. Архитектура

Поток (без изменений против mvp-дока):

```text
gateway.toml -> load -> validate -> DesiredState
  -> render {sing-box.json, nft.rules}
  -> plan (diff vs /var/lib/vpnrouter/current)
  -> apply --yes (v1) -> status/doctor -> rollback (v1)
```

Модули — **меньше, чем в deep-review**. Для spike десять модулей не нужны:

```text
src/
  main.rs      # lexopt dispatch, JSON на stdout, exit codes
  config.rs    # типы + TOML load + validate (+ warnings)
  render.rs    # sing-box JSON + nft rules text, чистые функции
  plan.rs      # Change/Risk, diff rendered vs current files
  error.rs     # ErrorEnvelope, коды
```

`validate.rs`, `desired.rs` отдельными файлами — когда нормализация
реально разойдётся с провалидированным конфигом; сейчас DesiredState ==
валидированный `GatewayConfig` c зафиксированным порядком policies.
`apply.rs`, `status.rs`, `doctor.rs` появляются в v1, под `#[cfg(target_os
= "linux")]` там, где Linux-specific.

Принципы (подтверждаю из доков + дополняю):

- config/validate/render/plan — чистые и portable; вся мутация в apply.
- **Детерминизм — тестируемый инвариант**: BTreeMap вместо HashMap где
  порядок влияет на вывод, golden-file тест, render дважды → идентичные байты.
- Только своя таблица `inet vpnrouter`; global flush запрещён навсегда.
- Rollback = restore last-good артефактов + re-apply их; routes/DNS
  rollback — v1+, когда apply станет реальным.
- Shell-out boundary (v1): `nft -c -f` / `nft -f`, `ip -j addr` /
  `ip -j route`, `systemctl reload-or-restart`, `sing-box check`.
  Netlink-крейты — только когда shell-out реально заболит.

Filesystem layout (v1):

```text
/etc/vpnrouter/gateway.toml
/var/lib/vpnrouter/current/{sing-box.json, nft.rules}
/var/lib/vpnrouter/last-good/{sing-box.json, nft.rules}
/var/lib/vpnrouter/plans/last-plan.json
```

**Дополнение, которого нет в доках — NAT.** Direct-политики означают, что
трафик клиентов уходит в WAN мимо туннеля — ему нужен masquerade. nft.rules
обязан содержать nat postrouting chain в нашей же таблице, а doctor —
проверять `net.ipv4.ip_forward=1`. Без этого "direct" в конфиге — фикция.

## 6. CLI / API contract

Spike: `schema`, `check --config`, `plan --config`, `detect-interfaces`.
V1: + `capabilities`, `apply --config --yes`, `status`, `doctor`,
`explain --source --dest --proto --port`, `rollback --yes`,
`resolve-subscription`. Later: `daemon`.

| Команда | Мутация | Root | Примечание |
| --- | --- | --- | --- |
| schema, capabilities | нет | нет | статика |
| check, plan, explain | нет | нет | чистые функции над конфигом/файлами |
| detect-interfaces | нет | нет | `ip -j addr` работает unprivileged |
| status | нет | частично | `nft list` требует root → деградировать с warning, не падать |
| doctor | нет (пишет bundle в /var/lib) | частично | как status |
| resolve-subscription (v1) | пишет кэш | нет | сеть, но не хост |
| apply, rollback (v1) | да | да | всегда `--yes`, всегда прогоняют plan-код первыми |

Вывод: **всегда JSON на stdout** (флаг `--json` принимается как no-op для
совместимости — человеческий формат добавим, когда появится человек,
который его попросит). Ошибки — тем же envelope на stdout + exit code.

Exit codes: `0` ok; `1` config/validation error; `2` environment error
(нет nft/ip/прав); `3` confirmation required (`--yes` отсутствует);
`4` apply failed (см. rollback-семантику в §8).

## 7. Config: gateway.toml

Spike-схема — **меньше, чем в deep-review**. Два отличия обоснованы:

1. **`[subscription]` из spike убран.** Fetch в spike недопустим (сеть),
   а держать секцию "declared but ignored" — враньё в схеме. В spike
   render эмитит один placeholder-outbound с тегом `vpn-out` и plan
   помечает risk `OUTBOUND_UNRESOLVED`. В v1 секция возвращается вместе с
   `resolve-subscription` (fetch отдельной командой, кэш, pin активного).
   Тогда же возвращаются `pinned_outbound`/`no_failover` — валидировать их
   в spike не против чего.
2. **Management bypass — явная секция, а не warning по имени policy.**
   Проверка "есть ли policy с 'management' в имени" (deep-review §12) —
   хрупкая эвристика. Явная секция и рендерится однозначно (accept в nft
   до kill-switch-правил + direct route в sing-box), и проверяется doctor'ом.

```toml
[interfaces]
wan = "eth0"
lan = "br0"
lan_cidr = "192.168.10.0/24"

[management]
sources = ["192.168.10.50/32"]   # всегда direct, всегда мимо killswitch
# ssh_port — отложен до v1 (первый потребитель — doctor)

[routing]
mode = "full"        # full | split

[tun]
mtu = 1420           # optional; validate 1280..=1500

# Policies first-match-wins в порядке файла: специфичные ПЕРЕД широкими.
# (office-default перед voice-udp сделал бы voice-udp мертвым правилом —
# validate ловит это warning'ом POLICY_SHADOWED.)
[[policies]]
name = "voice-udp"
source = "192.168.10.0/24"
protocol = "udp"
port = 50000
route = "vpn"        # vpn | direct | block

[[policies]]
name = "office-default"
source = "192.168.10.0/24"
route = "vpn"

[dns]
mode = "tunneled"    # tunneled | direct

[killswitch]
enabled = true
```

Валидация (spike):

- `wan != lan`; `lan_cidr` парсится; `management.sources` парсятся.
- policies непусты; имена уникальны; `route`/`mode`/`dns.mode` из enum;
  `port` требует `protocol`; `mtu` в 1280..=1500 (default 1420).
- `killswitch.enabled` требует ≥1 vpn-policy.
- Warning: `management.sources` пуст; policy source вне `lan_cidr`;
  первая matching policy для management-source — не direct (тень).
- Порядок policies = порядок в файле = порядок правил (first-match),
  зафиксировано документацией и тестом.

Отложено: destination CIDR/domain, pinned_outbound, no_failover,
subscription, несколько WAN/LAN, IPv6-политики (v1 — см. §8), DHCP
(не наша зона — doctor только проверяет адрес на LAN iface).

## 8. Safety

- **Никаких мутаций вне `apply --yes` / `rollback --yes`.** check/plan/
  status/doctor/explain физически не имеют кода записи в сеть.
- **apply всегда исполняет plan-код первым** и печатает его; отдельного
  "быстрого пути" нет.
- **SSH lockout:** при plan/apply читать `SSH_CLIENT`/`SSH_CONNECTION`;
  если source текущей сессии матчится в vpn/block-policy и не покрыт
  `[management].sources` — risk `SSH_MAY_DROP` (warning в plan, в apply v1
  — отказ без явного `--allow-ssh-risk`).
- **Fail-closed для forward, но с carve-out.** Отличие от desktop:
  kill-switch — это *статическое* правило в нашей таблице
  `iifname lan oifname wan ip saddr @vpn_routed drop`, стоящее всегда,
  пока killswitch enabled. В норме vpn-трафик уходит в tun и правило не
  срабатывает; при падении туннеля fallback-маршрут в WAN упирается в
  drop. Никакого runtime arm/disarm state-machine, как на desktop, —
  нечему сломаться. Management sources — accept выше drop. IPv6 forward
  от vpn-routed sources — drop по умолчанию (sing-box конфиг ipv4_only,
  v6 иначе утекает); явная v6-политика — later.
- **nft ownership:** только `table inet vpnrouter`; создание/замена
  атомарным `-f` файлом (flush нашей таблицы, не ruleset); удаление
  таблицы — единственный teardown; systemd `ExecStopPost` подчищает.
- **Rollback:** перед apply — copy current → last-good; неуспешный
  `nft -f` или неответивший после reload sing-box (in-band probe) →
  автоматический откат на last-good + exit 4 с отчётом, что откатилось.
- **Redaction (v1 doctor):** модель desktop-редактора + фикс дыры:
  значения сохраняются только под allowlisted-ключами — **включая числа**;
  URL → scheme://host; parse failure → файл omitted целиком; subscription
  URL — секрет (host-only в бандле); логи скрабятся до записи, не после.

## 9. Rust stack (проверено 2026-07-09)

crates.io + OSV, версии не изменились с проверки 06.07, advisories — ноль
на весь MVP-набор.

| Crate | Latest stable | Решение |
| --- | --- | --- |
| serde | 1.0.228 | spike |
| serde_json | 1.0.150 | spike |
| toml | 1.1.2+spec-1.1.0 | spike, `toml = "1"`; 0.9 только если MSRV/distro заставит |
| ipnet | 2.12.0 | spike, features=["serde"] |
| lexopt | 0.3.2 | spike; clap (4.6.1) не нужен для 6 подкоманд |
| ureq | 3.3.0 | v1, вместе с resolve-subscription |
| log | 0.4.33 | v1, вместе с runtime-логированием |
| systemd-journal-logger | 2.2.2 | v1, Linux-only target dep |
| schemars | 1.2.1 | skip: static schema string, генератор — если schema начнёт дрейфовать |
| tokio 1.52.3 / reqwest 0.13.4 | — | skip: async не нужен блокирующему CLI |
| notify 8.2.0, config 0.15.25, figment 0.10.19 | — | skip |
| YAML (serde_yml, yaml-rust) | — | **не брать**: RUSTSEC-2025-0068, RUSTSEC-2024-0320 |

## 10. First spike — точные задачи

1. `cargo init` в `D:\vibe-code\vpn-rust`; deps: serde, serde_json, toml,
   ipnet, lexopt — и всё.
2. `config.rs`: структуры §7 + load + validate → `Vec<ValidationError>` /
   `Vec<Warning>`.
3. `render.rs`: `render_sing_box(&cfg) -> String` (tun inbound stack=system,
   mtu clamp, route rules из policies в порядке файла + management direct
   первыми, dns по mode, placeholder outbound `vpn-out`);
   `render_nft(&cfg) -> String` (только `table inet vpnrouter`: forward
   chain c management accept + killswitch drop + v6 drop, nat postrouting
   masquerade).
4. `plan.rs`: diff rendered vs `/var/lib/vpnrouter/current/*` (отсутствие
   файла = change `create`), risks: `OUTBOUND_UNRESOLVED`,
   `NO_MANAGEMENT_BYPASS`, `SSH_MAY_DROP`.
5. `main.rs`: `schema` (include_str! статического JSON Schema), `check`,
   `plan`, `detect-interfaces` (`ip -j addr`; не-Linux →
   `UNSUPPORTED_PLATFORM`, exit 2).
6. `examples/gateway.toml` — sample из §7.
7. Тесты (один файл):
   - sample парсится, policies и management загружены;
   - невалидный конфиг (port без protocol; wan==lan) → структурная ошибка
     с кодом;
   - **render дважды → байт-в-байт идентично + golden file**;
   - nft render содержит `lan_cidr`, masquerade и **ни одной строки вне
     `table inet vpnrouter`**;
   - plan на пустом current → changes для sing-box и nftables + risk
     OUTBOUND_UNRESOLVED.

Success criteria: `cargo test` зелёный на Windows dev-машине; невалидный
конфиг → структурная ошибка; валидный → стабильный plan JSON; ни одна
команда не мутирует сеть; рендеры проходят ручную проверку `sing-box check`
/ `nft -c -f` на disposable Linux VM.

## 11. Kill criteria

Стоп или радикальное сжатие, если:

- plan не предсказывает apply однозначно (детерминизм сломался — умерла
  главная фича);
- SSH-lockout нельзя надёжно детектить → remote-использование запрещено →
  остаётся только локальный тул;
- nft-интеграция требует владеть всем host firewall (конфликт с firewalld
  /ufw неразрешим в своей таблице);
- gateway-режим sing-box конфига вырождается в копию desktop-сложности;
- сам владелец при реальной отладке не открывает `plan`/`doctor`, а лезет
  руками в nft/sing-box — значит нужен скрипт, а не агент;
- разговор снова уезжает в GUI/fleet/cloud до того, как локальный агент
  работает.

Главный критерий (аудитория = владелец): если через месяц реального
использования на своём шлюзе инструмент не экономит время против ручной
правки конфигов — сжать до генератора конфигов без apply или закрыть.
Рыночные проверки (тестеры, homeproxy-вопрос) актуальны только если
позже решим продуктизировать.

## 12. Next steps

**1 день:** spike 0 целиком (§10) — schema/check/plan + sample + тесты.

**1 неделя:** `detect-interfaces` на живом Linux; read-only `status`
(`ip -j`, `nft -j list table inet vpnrouter`, systemctl); `nft -c -f` и
`sing-box check` против рендеров на disposable VM; первый прогон
"конфиг → план → руками применить → работает ли NAT/forward вообще".

**1 месяц:** guarded `apply --yes` + last-good + `rollback --yes`;
in-band probe в status; redacted doctor bundle; SSH-lockout warning на
remote VM; `resolve-subscription`; решение — продукт или внутренний
appliance-тул (по поведению тестеров, см. kill criteria).

---

## 13. Лабораторная валидация (2026-07-09)

Spike 0 реализован (17 тестов, clippy -D warnings чистый) и его артефакты
проверены на живых системах — LXC-контейнеры на Proxmox VE 8.4
(pve-ninitux2): **vpnr-deb12** (CT 103, Debian 12, nftables 1.0.6) и
**vpnr-alpine** (CT 104, Alpine 3.23, nftables 1.1.5).

| Проверка | Debian 12 / nft 1.0.6 | Alpine 3.23 / nft 1.1.5 |
| --- | --- | --- |
| `nft -c -f` dry-run | OK | OK |
| `nft -f` реальная загрузка | OK | OK |
| Повторный `nft -f` поверх живой таблицы (atomic replace) | OK | OK |
| Содержимое таблицы в ядре соответствует рендеру | OK | OK |
| Глобальный ruleset не тронут (только `inet vpnrouter`) | OK | OK |
| `sing-box 1.13.14 check` на рендере | OK | OK (нужен gcompat) |

Выводы, влияющие на v1:

1. **Ядро канонизирует правила**: `192.168.10.50/32` → `192.168.10.50`,
   `priority 0` → `priority filter`. Значит `status`/`plan` в v1 не должны
   сравнивать `nft list` с рендером текстуально — diff только по нашим
   артефактам-файлам (что и спроектировано), а kernel-состояние проверять
   семантически (таблица есть/нет).
2. **Официальный бинарь sing-box — glibc-динамический**: на musl (Alpine)
   нужен `apk add gcompat`. Для v1-доки деплоя.
3. **Unprivileged LXC достаточно** для nft (свой netns): будущий CI может
   гонять apply-тесты в дешёвых непривилегированных контейнерах.
4. PVE 8.4 не создаёт CT Debian 13 («unsupported debian version») —
   ограничение лаборатории, не продукта.

## 14. apply / rollback: реализация и live-валидация (2026-07-09)

Реализованы `apply --yes` и `rollback --yes` ([apply.rs](../src/apply.rs)).
Порядок безопасности — контракт: (1) кандидат nft-правил проверяется
`nft -c -f` ДО любых изменений; (2) current бэкапится в last-good ДО замены;
(3) неудачная загрузка в ядро восстанавливает артефакты и перезагружает
прежние правила. **Apply конвергентен**: перезагружает свою таблицу даже без
файловых изменений — потерянная после ребута таблица чинится обычным
re-apply, без спец-команд. SSH-риск блокирует apply (exit 3
`SSH_RISK_REFUSED`) без явного `--allow-ssh-risk`. sing-box service
management сознательно отсутствует до resolve-subscription.

Юнит-тесты (26, fake-nft seam) + live-сценарий из 8 шагов в обоих CT
(Debian 12 и Alpine 3.23), всё зелёное: refuse без --yes → первый apply →
конвергенция → починка после «ребута» (delete table + re-apply) → backup
при изменении конфига → rollback → SSH-guard (отказ и override) →
глобальный ruleset не тронут.

Незапланированное живое подтверждение SSH-guard: `pct exec` наследует env
ssh-сессии хоста, и apply отказался применяться, увидев SSH-клиента
(192.168.0.x), не покрытого ни policy, ни management при `mode=full`, —
ровно тот сценарий lockout, ради которого guard написан.

Бинарь: статический musl (x86_64-unknown-linux-musl + rust-lld,
кросс-сборка с Windows, 1.4 MB) — один артефакт работает на glibc- и
musl-дистрибутивах без зависимостей.

## 15. status / doctor / explain + git-гейт (2026-07-09)

Реализована read-only тройка доверия ([status.rs](../src/status.rs), explain
в [plan.rs](../src/plan.rs)):

- `status [--config]` — артефакты (current/last-good/config_in_sync-дрейф),
  nft-таблица (binary/present/error), интерфейсы; host-пробы деградируют
  честно, команда не падает без root/не на Linux.
- `doctor --config` — checks-список: config+warnings, artifacts-дрейф,
  rollback-точка, nft-таблица, `net.ipv4.ip_forward` (читается напрямую из
  /proc, без shell-out), существование wan/lan интерфейсов. Live-прогон в CT
  немедленно поймал реальную проблему: `lan interface br0 not found` —
  ровно тот сигнал, ради которого doctor существует. Redaction-бандл
  сознательно отложен: в конфиге пока нет секретов; обязателен вместе с
  `[subscription]`.
- `explain --source [--dest --proto --port]` — детерминированный matcher,
  зеркалящий порядок render: management → policies (first-match) →
  routing.mode; отдаёт verdict + полный trace с причинами несовпадений +
  notes (killswitch-поведение, placeholder outbound). `--dest` принимается,
  но не оценивается (нет destination-политик в v1) — прямо сказано в notes.

Инженерный гейт: git-репозиторий (main), блокирующий pre-commit hook
(fmt --check + clippy -D warnings + cargo test; tracked-копия
`.githooks-pre-commit.sh`), `.gitattributes` с LF-нормализацией (golden
сравниваются побайтово, sh-скрипты ломаются на CRLF), проектный CLAUDE.md
с инвариантами. 33 юнит-теста. Полный /methodology-bootstrap (CI, ADR,
review-agent) сознательно не разворачивался: одиночный локальный проект,
hook покрывает принудительное ядро методологии.

## 16. resolve-subscription + реальный outbound + redaction + sing-box gate (2026-07-09)

Последний функциональный кусок v1 — шлюз стал реально подключаемым.

- **`resolve-subscription`** ([subscription.rs](../src/subscription.rs)):
  fetch (`ureq` 3, rustls-TLS) либо `--file` для офлайна → parse → выбор
  `active` → кэш `/var/lib/vpnrouter/subscription.json`. Парсер чистый
  (fixture-тестируемый), сеть — тонкий seam `Fetcher`. Форматы: base64/plain
  список `vless://` (свой tolerant base64-декодер, без новой зависимости) +
  passthrough готового sing-box JSON (лифт `outbounds`, отсев direct/
  selector/urltest). hysteria2/tuic/ss/vmess/trojan **отложены** (ponytail:
  `parse_uri` возвращает Unsupported, добавить когда реальная подписка
  принесёт). vless-парсер строит 1.13-outbound: reality (pbk/sid/utls),
  tls (sni/alpn/fp), flow, packet_encoding=xudp, транспорты ws/grpc/http.
- **Реальный outbound в render**: `render_sing_box(cfg, resolved)` — при
  наличии кэша ретегирует резолвнутый outbound в `vpn-out` (route-правила не
  меняются), иначе placeholder. `OUTBOUND_UNRESOLVED` risk только когда кэша
  нет. Golden стабилен: тесты рендерят с `None`.
- **Redaction** ([redact.rs](../src/redact.rs)): модель desktop-редактора —
  structured, ключи сохранены/значения маскированы, секретный ключ маскирует
  и **числовое** значение (фикс дыры desktop). URL → scheme://host, битый
  URL → `***`. reality public_key публичен и сохраняется. Применяется ко
  всему выводу `resolve-subscription`; реальные секреты — только на диске
  (root-only). Полный doctor-бандл — later, но механизм готов.
- **apply-гейт sing-box**: apply теперь валидирует ОБА кандидата
  (`nft -c` + `sing-box check`) ДО любых изменений (seam `DataPlane`).
  Отвергающий sing-box — жёсткий гейт (exit 4 `SINGBOX_CHECK_FAILED`),
  отсутствующий бинарь — reported skip. Плюс best-effort restart сервиса
  (`systemctl restart vpnrouter-sing-box`), reported, без health-loop/failover
  (тот урок HealthMonitor отложен в будущий daemon).
- systemd-unit + packaging/README.md (деплой, `CAP_NET_ADMIN`, gcompat для
  musl).

Крейт: `ureq = "3"` (rustls default). Следствие: `ring` требует C-тулчейн;
кросс-musl с Windows больше не собирается без `x86_64-linux-musl-gcc` —
Linux-бинарь теперь собирается **нативно** (доказано в CT: cargo 1.97 + gcc).

Валидация: 42 юнит-теста (base64 вектора, vless-reality парсинг, JSON
passthrough, select, redaction, render-with-resolved, sing-box-check гейт).
Нативная сборка + все тесты в Debian 12 CT. E2E из 8 шагов (реальный
vless-fixture → resolve → plan → apply → рендер реального outbound →
**настоящий `sing-box check` принял конфиг** → status/doctor → **живой
ureq-TLS фетч публичного HTTPS**), плюс негативный тест: битый reality
short_id → apply отказал (exit 4), рабочий `current/` не тронут — доказано,
что плохой резолв подписки не брикает шлюз.

Осталось (later, не v1-блокеры): hysteria2/tuic/ss URI, pinned_outbound/
failover, полный doctor redaction-бандл, daemon/watchdog, systemd-мониторинг.
