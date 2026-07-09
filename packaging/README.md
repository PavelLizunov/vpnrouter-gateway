# Deploying vpnrouter-gateway (v1, manual)

Linux gateway host, run as root (or a `CAP_NET_ADMIN` service).

## 1. Binary

Build natively on a Linux box (needs a C compiler: ureq's TLS pulls `ring`):

```sh
apt-get install -y gcc            # or: apk add gcc musl-dev
cargo build --release
install -m0755 target/release/vpnrouter-gateway /usr/local/bin/
```

Cross-compiling from Windows/macOS also works but requires a target C
toolchain (e.g. `x86_64-linux-musl-gcc` for the static musl target in
`.cargo/config.toml`); a native Linux build is the boring path.

## 2. sing-box data plane

Install the official `sing-box` (pinned 1.13.x) to `/usr/local/bin/sing-box`.
On musl/Alpine it needs glibc shim: `apk add gcompat`.

Install the service unit (does NOT auto-start; apply manages it):

```sh
install -m0644 packaging/vpnrouter-sing-box.service /etc/systemd/system/
systemctl daemon-reload
systemctl enable vpnrouter-sing-box
```

## 3. Config + forwarding

```sh
install -d /etc/vpnrouter
install -m0600 examples/gateway.toml /etc/vpnrouter/gateway.toml   # then edit
# forwarding must be on for a gateway (doctor checks this):
echo 'net.ipv4.ip_forward=1' > /etc/sysctl.d/99-vpnrouter.conf
sysctl -p /etc/sysctl.d/99-vpnrouter.conf
```

## 4. Resolve subscription, review, apply

```sh
vpnrouter-gateway resolve-subscription --config /etc/vpnrouter/gateway.toml
vpnrouter-gateway plan   --config /etc/vpnrouter/gateway.toml   # review changes + risks
vpnrouter-gateway apply  --config /etc/vpnrouter/gateway.toml --yes
vpnrouter-gateway status --config /etc/vpnrouter/gateway.toml
vpnrouter-gateway doctor --config /etc/vpnrouter/gateway.toml
```

`apply` validates both nft (`nft -c`) and sing-box config (`sing-box check`)
before touching anything, backs up the previous state, loads nft, and
best-effort restarts the sing-box service. `rollback --yes` restores the
previous state.

**SSH safety:** put your admin host in `[management].sources`. If you apply
over SSH from a host that a vpn/block policy would capture, apply refuses
unless you pass `--allow-ssh-risk`.
