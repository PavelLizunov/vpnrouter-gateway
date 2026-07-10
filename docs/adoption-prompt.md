# Adoption prompt

Вставить в сессию любого проекта/сервера, чтобы агент оценил, нужен ли ему
vpnrouter-gateway. Промпт самодостаточен.

---

Оцени, стоит ли добавлять в этот проект/на этот сервер инструмент
**vpnrouter-gateway**. Не продавай идею — проверяй её.

**Что это:** headless Linux-first VPN edge gateway (Rust, один бинарь,
6 зависимостей). Превращает декларативный `/etc/vpnrouter/gateway.toml` в
проверяемое состояние sing-box 1.13 (VLESS/Reality) + nftables (только
собственная таблица `inet vpnrouter`, killswitch на forward-hook, NAT):
`validate → render (детерминированный, golden-тесты) → plan (diff + риски,
включая SSH-lockout) → apply --yes (nft -c и sing-box check ДО изменений,