# ChasselFi production runbook

ChasselFi is a native Linux router service. Docker is useful for UI/API development, but production traffic enforcement requires host access to openNDS, nftables, dnsmasq, and `tc`.

## Supported topology

- WAN/admin: the untagged interface with the IPv4 default route.
- Customer LAN: VLAN 799 on that interface, `10.0.0.1/20`.
- Admin: `http://WAN_ADDRESS/admin/` (use HTTPS for any untrusted management network).
- Customer portal: `http://10.0.0.1/`.
- Hardware API: `http://10.0.0.1:2081`, restricted to `10.0.0.0/20` and authenticated per node.

Run the complete installer:

```bash
cd ~/ChasselFi
chmod +x deploy/install.sh deploy/*.sh deploy/router/*.sh
sudo deploy/router/install-vendo-vlan799.sh --wan enp2s0f0 --vlan-id 799
```

The switch port facing the server must carry VLAN 799 tagged. Enable client/AP isolation on every access point; a routed firewall cannot block traffic that the AP switches locally between wireless clients.

## Enforcement model

SQLite is authoritative for purchased time. ChasselFi accounts actual elapsed wall time, persists the accounting checkpoint, and reconciles every active/paused session with openNDS after service startup or restore. Pausing deauthorizes the client. Resuming reauthorizes it with the remaining time and speed.

The two bandwidth controls have different jobs:

- **Timer/default rate** is the per-client openNDS ceiling (for example 15 Mbps down and 15 Mbps up).
- **Global CAKE** is the aggregate WAN ceiling. Set it near 95% of the separately measured download and upload rates (for example 142 Mbps for a stable 150 Mbps direction), never to the 15 Mbps customer rate. ChasselFi uses `dual-dsthost nat` downstream and `dual-srchost nat` upstream for host fairness.

The Connected Users page reads `ndsctl json`, so it shows preauthenticated clients, gateway authentication state, IP, MAC, interface, live average throughput, and session byte totals. Pause/revoke is reported successful only after openNDS confirms that the client is no longer authenticated.

The router rules only forward IPv4 packets whose source belongs to `10.0.0.0/20`. This prevents unmanaged IPv6 from bypassing IPv4 captive enforcement. Keep the IPv6 policy set to **Block** until a fully managed dual-stack captive design is deployed.

After installation, open `http://neverssl.com` from an unpaid client. It must be redirected to the branded portal and must not reach the public internet. Then test voucher, coin, pause, resume, expiry, service restart during an active session, and device reconnect.

## Coin-node contract

Pair each ESP32, Arduino network bridge, Raspberry Pi, or Orange Pi in **Coin nodes**. The node gets a one-time key and does not receive internet access. Send the following to the pulse endpoint with `X-ChasselFi-Coin-Key`:

```json
{
  "nodeId": "vendo-front-01",
  "claimId": "customer-claim-uuid",
  "eventId": "boot42-pulse000012",
  "count": 1,
  "sequence": 12,
  "timestamp": 1786670000
}
```

`sequence` must increase across reboots (store it in NVS/EEPROM) and `timestamp` must be within five minutes. Enable **Protected coin messages** only after every deployed node supports these fields. Use hardware debounce, a watchdog, brownout detection, and an append-only local pulse queue so a temporary network outage does not lose paid coins.

## Operations and security

The **Operations** page checks ChasselFi, Nginx, dnsmasq, nftables, openNDS, its control socket, and CAKE. It also exposes the audit trail and gateway reconciliation.

```bash
sudo systemctl status chasselfi nginx dnsmasq nftables opennds
sudo ndsctl status
sudo nft list ruleset
sudo tc qdisc show
sudo journalctl -u chasselfi -u opennds --since today
```

Do not expose the admin dashboard directly to the internet. Use a management VLAN or WireGuard. For HTTPS, install a trusted certificate in Nginx, redirect only the admin hostname/address to HTTPS, and set `CHASSELFI_SECURE_COOKIES=1` in `/etc/chasselfi/chasselfi.env`.

Captive portals identify clients at the IP/MAC boundary. ChasselFi validates the IP/MAC pair reported by openNDS, refuses mismatches, and no longer restores a paid session by IP alone. A device that perfectly clones both an authorized IP and MAC is indistinguishable at this layer; prevent that on the access network with WPA2/WPA3-Enterprise or per-device PSKs plus managed-switch/AP client isolation, DHCP snooping, IP Source Guard, and Dynamic ARP Inspection where supported.

## Backup and recovery

The installer enables `chasselfi-backup.timer`. Daily backups are SQLite-consistent, integrity-checked, encrypted with `/etc/chasselfi/backup.key`, and checksummed. Retention follows the dashboard setting (30 days by default).

```bash
sudo systemctl start chasselfi-backup.service
sudo journalctl -u chasselfi-backup.service -n 50
sudo ls -lh /var/backups/chasselfi
```

Copy both encrypted backups and `backup.key` to separate secure storage. Losing the key makes the encrypted backup unrecoverable; storing only on the router is not disaster recovery.

Restore interactively:

```bash
sudo /usr/local/libexec/chasselfi-restore /var/backups/chasselfi/chasselfi-TIMESTAMP.tar.gz.enc
```

The restore verifies the checksum and SQLite integrity, preserves the current database under `/var/lib/chasselfi/recovery`, restores state/configuration, and starts the service. The web restore also verifies its SHA-256 payload and creates a pre-restore snapshot.

## Release validation

```bash
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test
node --check web/app.js
node --check web/portal.js
bash -n deploy/install.sh deploy/*.sh deploy/router/*.sh
nginx -t
```

Use a staging VLAN for upgrades. Take a backup, deploy, run the Operations checks, test an unpaid client, then test a short paid session through expiry. A UPS is strongly recommended for the server, switch, and access point.
