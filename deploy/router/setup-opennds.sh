#!/usr/bin/env bash
set -Eeuo pipefail

# Install and configure openNDS to use ChasselFi as its Forwarding
# Authentication Service (FAS). This is intentionally separate from the VLAN
# script because openNDS changes the LAN forwarding policy.

export PATH="/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin:${PATH:-}"
LAN_INTERFACE="${CHASSELFI_LAN:-}"
FAS_KEY="${CHASSELFI_FAS_KEY:-}"
FAS_PORT="${CHASSELFI_FAS_PORT:-2080}"
ASSUME_YES=0

usage() {
    cat <<'EOF'
Usage: setup-opennds.sh [options]

Options:
  --lan IFACE       Captive LAN interface (default: detected VLAN 799)
  --fas-key KEY     Shared key; otherwise read CHASSELFI_FAS_KEY or /etc/chasselfi/chasselfi.env
  --yes             Apply without an interactive confirmation
  -h, --help        Show this help

This configures openNDS to redirect clients to http://10.0.0.1:2080/portal/fas.
Run only after the VLAN/DHCP/NAT setup has been tested.
EOF
}

die() { echo "ERROR: $*" >&2; exit 1; }

while [[ $# -gt 0 ]]; do
    case "$1" in
        --lan) [[ $# -ge 2 ]] || die "--lan requires an interface"; LAN_INTERFACE="$2"; shift 2 ;;
        --fas-key) [[ $# -ge 2 ]] || die "--fas-key requires a value"; FAS_KEY="$2"; shift 2 ;;
        --yes) ASSUME_YES=1; shift ;;
        -h|--help) usage; exit 0 ;;
        *) die "Unknown option: $1" ;;
    esac
done

[[ "$EUID" -eq 0 ]] || die "run as root"
command -v apt-get >/dev/null 2>&1 || die "this installer currently targets Debian/Ubuntu"
command -v ip >/dev/null 2>&1 || die "ip command not found"

if [[ -z "$LAN_INTERFACE" ]]; then
    LAN_INTERFACE="$(ip -o link show | awk -F': ' '$2 ~ /\.799(@|$)/ {print $2; exit}' | cut -d@ -f1)"
fi
[[ -n "$LAN_INTERFACE" ]] || die "could not detect a VLAN 799 interface; pass --lan IFACE"
ip link show "$LAN_INTERFACE" >/dev/null 2>&1 || die "LAN interface does not exist: $LAN_INTERFACE"

if [[ -z "$FAS_KEY" && -r /etc/chasselfi/chasselfi.env ]]; then
    FAS_KEY="$(sed -n 's/^CHASSELFI_FAS_KEY=//p' /etc/chasselfi/chasselfi.env | sed 's/^\x27//;s/\x27$//;s/^"//;s/"$//' | head -n1)"
fi
[[ -n "$FAS_KEY" ]] || die "set CHASSELFI_FAS_KEY or pass --fas-key"
[[ "$FAS_PORT" =~ ^[0-9]+$ ]] && (( FAS_PORT >= 1024 && FAS_PORT <= 65535 )) \
    || die "CHASSELFI_FAS_PORT must be an unprivileged TCP port (1024-65535)"
[[ "$FAS_PORT" -ne 2050 ]] || die "CHASSELFI_FAS_PORT cannot use the openNDS gateway port 2050"

cat <<EOF
openNDS plan
  LAN interface: ${LAN_INTERFACE}
  Gateway:       10.0.0.1
  FAS URL:       http://10.0.0.1:${FAS_PORT}/portal/fas
  Security:      FAS level 1 (hashed client token)
EOF
if [[ "$ASSUME_YES" -ne 1 ]]; then
    read -r -p "Install and enable openNDS? [y/N] " answer
    [[ "$answer" =~ ^[Yy]$ ]] || { echo "Cancelled."; exit 0; }
fi

apt-get update
apt-get install -y opennds
command -v opennds >/dev/null 2>&1 || die "openNDS was not installed by this distribution"
command -v ndsctl >/dev/null 2>&1 || die "ndsctl was not installed with openNDS"
command -v runuser >/dev/null 2>&1 || die "runuser is required to verify privilege separation"
getent group chasselfi >/dev/null 2>&1 || die "install ChasselFi before configuring openNDS"

BACKUP_DIR="/var/backups/chasselfi-router/$(date +%Y%m%d-%H%M%S)"
mkdir -p "$BACKUP_DIR"
if [[ -f /etc/opennds/opennds.conf ]]; then
    cp -a /etc/opennds/opennds.conf "$BACKUP_DIR/opennds.conf"
fi
if [[ -f /etc/config/opennds ]]; then
    cp -a /etc/config/opennds "$BACKUP_DIR/opennds.uci"
fi

# Debian/Ubuntu's openNDS 10 package uses its UCI-style helper at runtime even
# when the legacy generic file is installed. Keep both representations in sync
# so the same installer also works with a generic Linux source build.
mkdir -p /etc/config /etc/opennds
cat >/etc/config/opennds <<EOF
config opennds 'main'
    option enabled '1'
    option gatewayinterface '${LAN_INTERFACE}'
    option gatewayaddress '10.0.0.1'
    option gatewayport '2050'
    option gatewayfqdn 'disable'
    option fasport '${FAS_PORT}'
    option faspath '/portal/fas'
    option fas_secure_enabled '1'
    option faskey '${FAS_KEY}'
    option login_option_enabled '0'
    option allow_preemptive_authentication '1'
    option preauthidletimeout '30'
    option authidletimeout '0'
    list users_to_router 'allow tcp port 53'
    list users_to_router 'allow udp port 53'
    list users_to_router 'allow udp port 67'
    list users_to_router 'allow tcp port 80'
    list users_to_router 'allow tcp port ${FAS_PORT}'
    list users_to_router 'allow tcp port 2081'
EOF

cat >/etc/opennds/opennds.conf <<EOF
# ChasselFi generic Linux openNDS configuration.
GatewayInterface ${LAN_INTERFACE}
GatewayPort 2050
GatewayFQDN disable
fasport ${FAS_PORT}
faspath /portal/fas
fas_secure_enabled 1
faskey ${FAS_KEY}
login_option_enabled 0
AllowPreemptiveAuthentication 1
preauthidletimeout 30
authidletimeout 0

FirewallRuleSet users-to-router {
    FirewallRule allow udp port 53
    FirewallRule allow tcp port 53
    FirewallRule allow udp port 67
    FirewallRule allow tcp port 80
    FirewallRule allow tcp port ${FAS_PORT}
    FirewallRule allow tcp port 2081
}
EOF

if command -v systemctl >/dev/null 2>&1; then
    install -D -o root -g root -m0755 \
        "$(dirname -- "${BASH_SOURCE[0]}")/opennds-socket-permissions.sh" \
        /usr/local/libexec/chasselfi-opennds-socket-permissions
    install -d -m0755 /etc/systemd/system/opennds.service.d
    cat >/etc/systemd/system/opennds.service.d/chasselfi.conf <<'EOF'
[Service]
ExecStartPost=/usr/local/libexec/chasselfi-opennds-socket-permissions
EOF
    systemctl daemon-reload
    systemctl enable opennds
    systemctl reset-failed opennds 2>/dev/null || true
    systemctl restart opennds
    sleep 4
    if ! systemctl is-active --quiet opennds; then
        journalctl -u opennds -n 80 --no-pager >&2 || true
        die "openNDS did not stay active"
    fi
    if ! journalctl -u opennds --since '-30 seconds' --no-pager \
        | grep -Fq "Attempting to Bind to interface: ${LAN_INTERFACE}"; then
        journalctl -u opennds -n 80 --no-pager >&2 || true
        die "openNDS did not bind to ${LAN_INTERFACE}"
    fi
    if ! runuser -u chasselfi -- ndsctl status >/dev/null 2>&1; then
        ls -l /tmp/ndsctl.sock >&2 || true
        die "the unprivileged ChasselFi service cannot reach the openNDS control socket"
    fi
    systemctl --no-pager --full status opennds || true
fi

echo
echo "openNDS FAS integration enabled."
echo "Generate a voucher in ChasselFi, connect a VLAN client, and open an HTTP URL."
echo "Inspect logs with: journalctl -u opennds -f"
