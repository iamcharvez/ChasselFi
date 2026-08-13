#!/usr/bin/env bash
set -Eeuo pipefail

# Install and configure openNDS to use ChasselFi as its Forwarding
# Authentication Service (FAS). This is intentionally separate from the VLAN
# script because openNDS changes the LAN forwarding policy.

export PATH="/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin:${PATH:-}"
LAN_INTERFACE="${CHASSELFI_LAN:-}"
FAS_KEY="${CHASSELFI_FAS_KEY:-}"
ASSUME_YES=0

usage() {
    cat <<'EOF'
Usage: setup-opennds.sh [options]

Options:
  --lan IFACE       Captive LAN interface (default: detected VLAN 799)
  --fas-key KEY     Shared key; otherwise read CHASSELFI_FAS_KEY or /etc/chasselfi/chasselfi.env
  --yes             Apply without an interactive confirmation
  -h, --help        Show this help

This configures openNDS to redirect clients to http://10.0.0.1/portal/fas.
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

cat <<EOF
openNDS plan
  LAN interface: ${LAN_INTERFACE}
  Gateway:       10.0.0.1
  FAS URL:       http://10.0.0.1/portal/fas
  Security:      FAS level 1 (hashed client token)
EOF
if [[ "$ASSUME_YES" -ne 1 ]]; then
    read -r -p "Install and enable openNDS? [y/N] " answer
    [[ "$answer" =~ ^[Yy]$ ]] || { echo "Cancelled."; exit 0; }
fi

apt-get update
apt-get install -y opennds
command -v opennds >/dev/null 2>&1 || die "openNDS was not installed by this distribution"

CONFIG_FORMAT=""
if [[ -f /etc/opennds/opennds.conf ]]; then
    CONFIG_FILE="/etc/opennds/opennds.conf"
    CONFIG_FORMAT="generic"
elif [[ -f /etc/config/opennds ]]; then
    CONFIG_FILE="/etc/config/opennds"
    CONFIG_FORMAT="uci"
else
    die "openNDS config not found; expected /etc/opennds/opennds.conf or /etc/config/opennds"
fi
BACKUP_DIR="/var/backups/chasselfi-router/$(date +%Y%m%d-%H%M%S)"
mkdir -p "$BACKUP_DIR"
cp -a "$CONFIG_FILE" "$BACKUP_DIR/opennds"

if [[ "$CONFIG_FORMAT" == "generic" ]]; then
cat >"$CONFIG_FILE" <<EOF
# ChasselFi generic Linux openNDS configuration.
GatewayInterface ${LAN_INTERFACE}
GatewayPort 2050
GatewayFQDN disable
fasport 80
fasremoteip 10.0.0.1
faspath /portal/fas
fas_secure_enabled 1
faskey ${FAS_KEY}
login_option_enabled 0

FirewallRuleSet users-to-router {
    FirewallRule allow udp port 53
    FirewallRule allow tcp port 53
    FirewallRule allow udp port 67
    FirewallRule allow tcp port 80
}
EOF
else
cat >"$CONFIG_FILE" <<EOF
config opennds 'main'
    option gatewayinterface '${LAN_INTERFACE}'
    option gatewayaddress '10.0.0.1'
    option gatewayport '2050'
    option fasport '80'
    option faspath '/portal/fas'
    option fas_secure_enabled '1'
    option faskey '${FAS_KEY}'
    option login_option_enabled '0'
EOF
fi

if command -v systemctl >/dev/null 2>&1; then
    systemctl enable opennds
    systemctl restart opennds
    systemctl --no-pager --full status opennds || true
fi

echo
echo "openNDS FAS integration enabled."
echo "Generate a voucher in ChasselFi, connect a VLAN client, and open an HTTP URL."
echo "Inspect logs with: journalctl -u opennds -f"
