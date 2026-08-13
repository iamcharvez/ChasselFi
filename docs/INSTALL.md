# ChasselFi installation

This guide targets Debian or Ubuntu Linux with systemd. The recommended production layout is:

- ChasselFi Rust service on `127.0.0.1:8080`
- Nginx on ports 80/443
- `enp2s0f0` (or the default-route NIC) as the untagged WAN
- VLAN 799 on that same NIC as the customer LAN
- client gateway `10.0.0.1/20`

## Prerequisites

Install the build and runtime tools:

```bash
sudo apt update
sudo apt install -y build-essential curl git nginx openssl pkg-config
# `passwd` provides groupadd/useradd, which the automated installer uses.
sudo apt install -y passwd
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
source "$HOME/.cargo/env"
```

Clone the project and enter it:

```bash
git clone <your-repository-url> chasselfi
cd chasselfi
```

## Automated installation

The installer builds the release binary, creates the restricted `chasselfi` user, creates `/var/lib/chasselfi`, installs the systemd unit, generates credentials when needed, and optionally installs the Nginx proxy:

The automated installer requires Debian/Ubuntu with systemd and the `groupadd`, `useradd`, and `getent` commands. Alpine, OpenWrt, and other non-systemd systems need a separate service definition or a Debian/Ubuntu VM/PC.

```bash
chmod +x deploy/install.sh
sudo CHASSELFI_ADMIN_PASSWORD='replace-with-a-long-password' \
  ./deploy/install.sh --with-nginx
```

If you omit the password, the installer generates one and prints it once. Store it securely.

New installations start with real empty sales, session, voucher, and blocked
site data plus the default rate packages. ChasselFi does not seed fictional
sales or clients.

Check the service:

```bash
sudo systemctl status chasselfi
curl http://127.0.0.1:8080/api/health
```

## Manual installation

### 1. Build and install the binary

```bash
cargo build --release
sudo install -Dm755 target/release/chasselfi /usr/local/bin/chasselfi
```

### 2. Create the service account and directories

```bash
sudo groupadd --system chasselfi || true
sudo useradd --system --gid chasselfi --home-dir /var/lib/chasselfi \
  --create-home --shell /usr/sbin/nologin chasselfi || true
sudo install -d -o chasselfi -g chasselfi -m0750 /var/lib/chasselfi
sudo cp -a web/. /var/lib/chasselfi/web/
sudo chown -R root:chasselfi /var/lib/chasselfi/web
sudo find /var/lib/chasselfi/web -type d -exec chmod 0755 {} \;
sudo find /var/lib/chasselfi/web -type f -exec chmod 0644 {} \;
sudo install -d -o root -g chasselfi -m0750 /etc/chasselfi
sudo install -o root -g chasselfi -m0640 \
  deploy/chasselfi-config.json.example /etc/chasselfi/config.json
```

Edit `/etc/chasselfi/config.json` and keep the service bound behind Nginx:

```json
{
  "listen": "127.0.0.1:8080",
  "data_dir": "/var/lib/chasselfi",
  "hardware_mode": "simulated"
}
```

`simulated` is the recommended mode while this PC is only running the server. It keeps reboot, shutdown, and router operations non-destructive. Switch to `linux` only when this host is intentionally becoming the gateway and the reviewed data-plane configuration is ready.

Create the protected environment file:

```bash
sudo install -o root -g chasselfi -m0640 /dev/null /etc/chasselfi/chasselfi.env
sudo sh -c 'printf "%s\n" \
  "CHASSELFI_ADMIN_USER=admin" \
  "CHASSELFI_ADMIN_PASSWORD=replace-with-a-long-password" \
  "CHASSELFI_FAS_KEY=$(openssl rand -hex 32)" \
  "CHASSELFI_SECURE_COOKIES=1" \
  > /etc/chasselfi/chasselfi.env'
```

### 3. Install and start systemd

```bash
sudo install -o root -g root -m0644 \
  deploy/chasselfi.service /etc/systemd/system/chasselfi.service
sudo systemctl daemon-reload
sudo systemctl enable --now chasselfi
sudo journalctl -u chasselfi -f
```

### 4. Install Nginx

```bash
sudo install -Dm0644 deploy/nginx/chasselfi.conf \
  /etc/nginx/conf.d/chasselfi.conf
sudo nginx -t
sudo systemctl enable --now nginx
sudo systemctl reload nginx
```

Add TLS before exposing the dashboard outside a trusted network. After certificates are installed, enable the HTTPS server block, redirect HTTP to HTTPS, and keep `CHASSELFI_SECURE_COOKIES=1`.

### 5. Review PC-server WAN/LAN discovery

The Network status page now inspects the server without changing its networking:

1. The interface owning the default route is recommended as **WAN**.
2. An active VLAN 799 interface with `10.0.0.1/20` is preferred as **LAN**.
3. For legacy two-NIC layouts, a USB Ethernet adapter is the next LAN choice.
4. If neither exists, the best remaining Ethernet interface without a default route is shown with medium confidence.

Use **Generate review plan** to validate the mapping and see the exact `ip`, `sysctl`, and `nft` commands. In PC/server mode the plan is never executed automatically, because changing the active interface can disconnect the administrator. The default proposed client gateway is `10.0.0.1/20`.

If ChasselFi is running in Docker bridge mode, discovery is limited to the container's virtual NIC. Use the native systemd installation for physical adapter discovery; do not add `--privileged` just to make the dashboard see hardware.

### 6. Configure the Linux router data plane

Identify interfaces first:

```bash
ip -br link
ip -br address
```

Then review and adapt:

- `deploy/router/dnsmasq-chasselfi.conf`
- `deploy/router/nftables-chasselfi.nft`

The templates do not apply automatically. Replace the WAN/LAN placeholders, validate firewall rules from a maintenance console, and test one client for DHCP, gateway ping, DNS, and internet access before enabling live shaping.

## Upgrading

Back up `/var/lib/chasselfi/chasselfi.sqlite3`, stop the service, install the new binary, and restart:

```bash
sudo cp /var/lib/chasselfi/chasselfi.sqlite3 \
  "/var/lib/chasselfi/chasselfi.sqlite3.$(date +%Y%m%d%H%M%S).bak"
sudo systemctl stop chasselfi
cargo build --release
sudo install -Dm755 target/release/chasselfi /usr/local/bin/chasselfi
sudo systemctl start chasselfi
```

When updating a Git checkout, stop if `git pull` reports an error. Running the
installer after an aborted pull reinstalls the old code. If Cargo generated a
local lock-file change and you do not intend to keep it, preserve a copy and
restore only that file before pulling:

```bash
cd ~/ChasselFi
cp -a Cargo.lock "$HOME/Cargo.lock.before-update.$(date +%Y%m%d-%H%M%S)"
git restore -- Cargo.lock
git pull --ff-only origin main
git status --short
```

Do not use `git reset --hard` for this. Review any other modified files rather
than discarding them.

## Troubleshooting

```bash
sudo systemctl status chasselfi
sudo journalctl -u chasselfi --since today
sudo nginx -t
curl -i http://127.0.0.1:8080/api/health
```

If the portal works but clients have no internet, the issue is in the Linux data plane—interface assignment, DHCP, forwarding, NAT, DNS, or firewall rules—not the dashboard process.

If a phone shows the blue openNDS **Accept Terms of Service** page, FAS is not
active. Re-run the current `setup-opennds.sh` and confirm its plan contains
port `2080`. The script reads back the effective openNDS options and fails if
the package ignored them:

```bash
sudo bash deploy/router/setup-opennds.sh --lan enp2s0f0.799 --yes
sudo /bin/bash /usr/lib/opennds/libopennds.sh get_option_from_config fasport
sudo /bin/bash /usr/lib/opennds/libopennds.sh get_option_from_config faspath
```

The two values must be `2080` and `/portal/fas`. Then forget and reconnect the
customer Wi-Fi network (or toggle Wi-Fi off/on) to discard the phone's cached
captive portal session.

## Enable real voucher enforcement

The normal dashboard portal is useful for local testing, but a production
customer network must be gated by a captive-portal engine. After the VLAN
router has been tested with a client, install openNDS and connect its
Forwarding Authentication Service (FAS) to ChasselFi:

```bash
chmod +x deploy/router/setup-opennds.sh
sudo bash deploy/router/setup-opennds.sh --lan enp2s0f0.799
```

The script writes both supported openNDS configuration formats with FAS
security level 1 and points clients internally to
`http://10.0.0.1:2080/portal/fas`. Port 80 remains the normal branded customer
portal. The FAS endpoint validates a ChasselFi voucher or routes the customer
to Coin mode, records the confirmed payment, creates the paid session, and
returns the hashed authentication token to openNDS. openNDS then enforces the
session at the forwarding boundary and expires it according to the purchased
minutes.

For an ESP32, Arduino, Orange Pi, GPIO, or serial coin acceptor, complete
[the coin-node setup](COIN_NODES.md). Network nodes use the authenticated
local-only API on `10.0.0.1:2081`; they are not given internet access.

For the complete VLAN 799 installation in one command:

```bash
chmod +x deploy/install.sh deploy/router/*.sh
sudo bash deploy/router/install-vendo-vlan799.sh --wan enp2s0f0
```

After installation, the administrator uses `http://<WAN-IP>/admin/` and VLAN
799 clients use `http://10.0.0.1/`. Test captive detection with a newly joined
client and `http://neverssl.com`; internet forwarding must remain blocked until
a valid voucher or confirmed physical coin purchase is accepted.

Do not leave the unrestricted nftables forwarding rule active while testing a
portal: openNDS must be the component deciding which clients may forward to
the WAN. Check `journalctl -u opennds -f` and `journalctl -u chasselfi -f` when
testing a client.
