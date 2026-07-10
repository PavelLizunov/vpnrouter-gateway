# vpnrouter-gateway — короткий промпт для оценки

Вставь это в другой проект/AI, чтобы он понял, что это, и решил — стоит ли добавлять.

---

## Что это (2–3 строки)

**vpnrouter-gateway** — headless Linux-first шлюз, который превращает один
конфиг `gateway.toml` в проверяемое состояние сети (sing-box + nftables) с
`plan` до применения, безопасным `apply` и `rollback`. Один Rust-бинарь,
5 зависимостей, вывод — строгий JSON. Не «VPN с кнопкой», а инструмент, где
любое изменение сети предсказуемо, объяснимо и откатываемо.

## Поток

```
gateway.toml → validate → render (sing-box 1.13 JSON + nftables) →
plan → apply --yes → status / doctor / explain → rollback
```

## Чем отличается (одной фразой)
- от **desktop-VPN** — вся LAN, без GUI, headless;
- от **sing-box вручную** — plan/rollback/doctor + оркестрация nft/NAT/DNS;
- от **OpenWrt homeproxy** — на любом systemd-Linux, JSON-CLI, plan-before-apply;
- от **Tailscale** — про egress-политику через коммерческую подписку, не про mesh.

## Ключевые инварианты (почему ему можно доверять)
- Детерминизм render байт-в-байт (golden-тесты).
- nftables только своя таблица `inet vpnrouter`; `flush ruleset` запрещён.
- Мутации только в `apply`/`rollback` и только с `--yes`; остальное read-only.
- SSH-guard: apply откажется, если применение отрежет текущую SSH-сессию.
- apply валидирует nft (`nft -c`) и sing-box (`check`) ДО изменений; плохой
  конфиг/подписка не может забрикать шлюз (доказано live-тестом).

## Статус
v1 CLI закончен. 45 тестов, clippy `-D warnings` + fmt чистые, блокирующий
pre-commit hook. Живая валидация на Proxmox (Debian 12 / Alpine 3.23) и на
реальной подписке: узлы резолвятся, sing-box 1.13.14 принимает конфиг,
трафик выходит через нужную страну. Репо: github.com/PavelLizunov/vpnrouter-gateway

## Стоит ли добавлять? — критерий
Бери, если нужен **декларативный, откатываемый edge-шлюз на обычном Linux**
с sing-box-протоколами (VLESS/Reality, Hysteria2, TUIC) и машинным JSON-API.
Не бери, если хватает «скопировать sing-box config на роутер» один раз руками,
или если целевая платформа — прошивка (OpenWrt homeproxy закроет быстрее).
