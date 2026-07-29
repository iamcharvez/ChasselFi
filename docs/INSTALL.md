# ChasselFi installation

This guide targets Debian or Ubuntu Linux with systemd. The recommended production layout is:

- ChasselFi Rust service on `127.0.0.1:8080`
- Nginx on ports 80/443
- built-in Ethernet as WAN
- USB-to-LAN as the client LAN
- client gateway `10.0.0.1/20`

## Prerequisites

Install the build and runtime tools:

```bash
sudo apt update
sudo apt install -y build-essential curl git nginx openssl pkg-config
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

```bash
chmod +x deploy/install.sh
sudo CHASSELFI_ADMIN_PASSWORD='replace-with-a-long-password' \
  ./deploy/install.sh --with-nginx
```

If you omit the password, the installer generates one and prints it once. Store it securely.

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
2. A second Ethernet adapter whose sysfs path contains `usb` is preferred as **LAN** (ideal for a USB-to-LAN adapter).
3. If no USB adapter is found, the best remaining Ethernet interface without a default route is shown with medium confidence.

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

## Troubleshooting

```bash
sudo systemctl status chasselfi
sudo journalctl -u chasselfi --since today
sudo nginx -t
curl -i http://127.0.0.1:8080/api/health
```

If the portal works but clients have no internet, the issue is in the Linux data plane—interface assignment, DHCP, forwarding, NAT, DNS, or firewall rules—not the dashboard process.
