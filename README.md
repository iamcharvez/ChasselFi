# ChasselFi Piso WiFi

A Rust-powered Piso WiFi management system with a responsive operator dashboard and customer captive portal. It is designed to run on a small Linux router or single-board computer while remaining safe to demo on a development machine.

## Included in this MVP

- Live dashboard with sales, clients, uptime, CPU, memory, and seven-day revenue
- Timer-rate CRUD with per-package upload/download limits
- Voucher batch generation, copy, deletion, and one-time redemption
- Sales ledger with search and CSV export
- Client session pause, resume, and termination controls
- Site-block deny-list management
- Portal branding, feature flags, speed limits, and maintenance settings
- Mobile-first customer portal with coin-payment simulation and voucher entry
- Locally vendored Bootstrap 5.3.8 components and responsive utilities
- Chart.js revenue visualization with responsive tooltips and hover states
- SQLite persistence in `data/chasselfi.sqlite3` with automatic migration from the legacy Bantay database/JSON store
- Argon2 administrator login, HttpOnly sessions, and CSRF protection for admin writes
- Login throttling, security response headers, and eight-hour admin session expiry
- Backup download/restore and printable ready-voucher sheets
- Automatic session countdown enforcement with device-bound portal heartbeats
- Business snapshot metrics for average sale, coin/voucher mix, inventory value, and active clients
- Safe simulated hardware mode; reboot and shutdown never touch the host by default
- PC/server network discovery: default-route WAN detection, USB-Ethernet LAN recommendation, and reviewed (non-applied) network plans

## Run locally

For a Linux server, see the complete [manual and automated installation guide](docs/INSTALL.md).

Install the stable Rust toolchain, then:

```bash
cargo run --release
```

Open:

- Admin dashboard: <http://localhost:8080>
- Customer portal: <http://localhost:8080/portal.html>
- Health check: <http://localhost:8080/api/health>

The default listener is `0.0.0.0:8080`. Copy `config.example.json` to `config.json` to customize it, or point `CHASSELFI_CONFIG` at another file. `BANTAY_CONFIG` remains accepted as a compatibility alias.

Set these environment variables before deployment:

```bash
CHASSELFI_ADMIN_USER=admin
CHASSELFI_ADMIN_PASSWORD="use-a-long-unique-password"
CHASSELFI_SECURE_COOKIES=1
# Keep unset or 0 while validating a router. Set to 1 only after reviewing tc commands.
CHASSELFI_LIVE_ROUTER=0
```

## Nginx production edge

For a Linux deployment, place ChasselFi behind the included Nginx reverse proxy:

```bash
sudo cp deploy/nginx/chasselfi.conf /etc/nginx/conf.d/chasselfi.conf
sudo nginx -t
sudo systemctl reload nginx
```

Use `deploy/chasselfi-config.json.example` so the Rust service listens only on
`127.0.0.1:8080`; Nginx becomes the public HTTP/HTTPS edge. Add TLS and a
management-subnet firewall rule before exposing the dashboard beyond the LAN.

## Container

```bash
docker build -t chasselfi .
docker run --rm -p 8080:8080 \
  -e CHASSELFI_ADMIN_PASSWORD="use-a-long-unique-password" \
  -v chasselfi-data:/app/data chasselfi
```

The image includes `tc` and `nft` so the router adapter can report tool
availability and generate reviewed plans. It deliberately runs as an
unprivileged user; do not add blanket `--privileged` access. For a real router,
run the web service natively beside a small, allow-listed root networking
helper (or provide an equivalent audited deployment-specific integration).
In the default bridge network, the dashboard can only discover Docker's virtual
interfaces; use the native systemd install (or an explicitly reviewed host
network setup) to identify the PC's physical WAN and USB-LAN adapters.

## PC server first, router later

ChasselFi can run on a normal Linux PC as the management server first. The Network status page identifies the likely WAN and LAN and generates a reviewable plan, but it does not reconfigure the PC. This keeps the dashboard usable while you verify the physical topology. When you later turn the PC into the gateway, apply the reviewed `deploy/router/` templates from a maintenance console and test DHCP, DNS, forwarding, NAT, and one client before enabling live traffic changes.

## Linux router deployment

The admin product and data plane are intentionally separated. The repository now ships a safe Linux router adapter with a dry-run shaping plan, real Linux interface telemetry, and reviewed WAN/LAN templates under `deploy/router/`. Before setting `hardware_mode: "linux"` and `CHASSELFI_LIVE_ROUTER=1` on a real vendo, validate the target-specific data plane for:

- `nftables` NAT, captive-portal redirects, client allow-listing, and blocked hosts
- `dnsmasq` DHCP/DNS on the captive LAN
- `hostapd` access-point lifecycle
- `tc`/CAKE per-client shaping
- GPIO or serial coin pulse input with debounce and relay output
- Privilege separation: keep the web process unprivileged and expose only allow-listed operations through a root helper

The DNS and firewall templates are intentionally not auto-applied. Replace the
placeholder interface names, review the rules, and test with one client before
using them on the live router.

For the included systemd unit, install the binary at `/usr/local/bin/chasselfi`,
place its config at `/etc/chasselfi/config.json`, and set `data_dir` to `.`;
systemd creates `/var/lib/chasselfi` as the writable state directory.

The admin dashboard requires the configured credentials. The customer portal keeps only the package list, voucher redemption, and validated package purchase endpoints public.

Do not expose the admin dashboard to the public internet without adding authentication, TLS, CSRF protection, rate limiting, and a firewall rule that limits admin access to a trusted management network.

## Data model

All write operations are persisted locally in `data/chasselfi.sqlite3`. A legacy
`bantay.sqlite3` or `store.json` file is imported automatically on first start; keep the SQLite
file and use the authenticated backup endpoint for routine recovery.

## Project layout

```text
src/            Rust API, SQLite persistence, auth, host metrics, router adapter, safe system actions
web/            Bootstrap admin SPA, Chart.js graphs, and customer captive portal
deploy/         Nginx edge, systemd service, and reviewed Linux WAN/LAN templates
config.example.json
Dockerfile
```
