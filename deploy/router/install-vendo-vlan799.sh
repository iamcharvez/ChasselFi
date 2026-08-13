#!/usr/bin/env bash
set -Eeuo pipefail

export PATH="/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin:${PATH:-}"
SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_DIR="$(cd -- "${SCRIPT_DIR}/../.." && pwd)"
WAN_INTERFACE="${CHASSELFI_WAN:-}"
VLAN_ID="${CHASSELFI_VLAN_ID:-799}"
ASSUME_YES=0

usage() {
    cat <<'EOF'
Usage: sudo bash deploy/router/install-vendo-vlan799.sh [options]

Options:
  --wan IFACE       Untagged WAN interface (default: default-route interface)
  --vlan-id ID      Tagged customer VLAN (default: 799)
  --yes             Skip the final confirmation
  -h, --help        Show this help

Set CHASSELFI_ADMIN_PASSWORD before running, or the script securely prompts
for it. The switch port facing the server must carry the customer VLAN tagged.
EOF
}

die() { echo "ERROR: $*" >&2; exit 1; }

while [[ $# -gt 0 ]]; do
    case "$1" in
        --wan) WAN_INTERFACE="${2:-}"; shift 2 ;;
        --vlan-id) VLAN_ID="${2:-}"; shift 2 ;;
        --yes) ASSUME_YES=1; shift ;;
        -h|--help) usage; exit 0 ;;
        *) die "unknown option: $1" ;;
    esac
done

[[ "$EUID" -eq 0 ]] || die "run this script with sudo"
[[ -f "$PROJECT_DIR/Cargo.toml" ]] || die "run the script from the ChasselFi repository"

if [[ -z "$WAN_INTERFACE" ]]; then
    WAN_INTERFACE="$(ip -4 route show default | awk 'NR == 1 {print $5}')"
fi
[[ -n "$WAN_INTERFACE" ]] || die "no IPv4 default-route interface found; pass --wan IFACE"

# sudo commonly changes HOME to /root even when Rust belongs to the login
# user. Locate that user's rustup installation without modifying the system.
if ! command -v cargo >/dev/null 2>&1; then
    cargo_home=""
    if [[ -n "${SUDO_USER:-}" && "$SUDO_USER" != root ]]; then
        user_home="$(getent passwd "$SUDO_USER" | cut -d: -f6)"
        [[ -x "$user_home/.cargo/bin/cargo" ]] && cargo_home="$user_home"
    fi
    if [[ -z "$cargo_home" ]]; then
        cargo_path="$(find /home -maxdepth 4 -path '*/.cargo/bin/cargo' -print -quit 2>/dev/null || true)"
        [[ -n "$cargo_path" ]] && cargo_home="${cargo_path%/.cargo/bin/cargo}"
    fi
    [[ -n "$cargo_home" ]] || die "Rust was not found; install rustup for the login user first"
    export HOME="$cargo_home" CARGO_HOME="$cargo_home/.cargo" RUSTUP_HOME="$cargo_home/.rustup"
    export PATH="$CARGO_HOME/bin:$PATH"
fi

if [[ -z "${CHASSELFI_ADMIN_PASSWORD:-}" ]]; then
    read -r -s -p "New ChasselFi administrator password: " first_password
    echo
    read -r -s -p "Confirm administrator password: " second_password
    echo
    [[ ${#first_password} -ge 12 ]] || die "administrator password must contain at least 12 characters"
    [[ "$first_password" == "$second_password" ]] || die "passwords did not match"
    export CHASSELFI_ADMIN_PASSWORD="$first_password"
fi

cat <<EOF
ChasselFi production vendo plan
  WAN/admin:       ${WAN_INTERFACE}
  Customer VLAN:  ${WAN_INTERFACE}.${VLAN_ID}
  Customer portal:http://10.0.0.1/
  Admin dashboard: http://$(ip -4 -o addr show dev "$WAN_INTERFACE" | awk 'NR == 1 {split($4,a,"/"); print a[1]}')/admin/
  Gateway/DHCP:    10.0.0.1/20, 10.0.0.100-10.0.15.250
EOF
if [[ "$ASSUME_YES" -ne 1 ]]; then
    read -r -p "Install and activate this router configuration? [y/N] " answer
    [[ "$answer" =~ ^[Yy]$ ]] || { echo "Cancelled."; exit 0; }
fi

chmod +x "$PROJECT_DIR/deploy/install.sh" "$SCRIPT_DIR"/*.sh
"$PROJECT_DIR/deploy/install.sh" --with-nginx
"$SCRIPT_DIR/setup-vlan799.sh" --wan "$WAN_INTERFACE" --vlan-id "$VLAN_ID" --yes

# The native router installation intentionally enables real, authenticated
# system controls. The general-purpose installer keeps simulation as default.
sed -i 's/"hardware_mode"[[:space:]]*:[[:space:]]*"simulated"/"hardware_mode": "linux"/' \
    /etc/chasselfi/config.json
systemctl restart chasselfi nginx

"$SCRIPT_DIR/setup-opennds.sh" --lan "${WAN_INTERFACE}.${VLAN_ID}" --yes

curl --fail --silent --show-error http://127.0.0.1:8080/api/health >/dev/null
curl --fail --silent --show-error http://10.0.0.1/portal.html >/dev/null
curl --fail --silent --show-error http://10.0.0.1:2080/styles.css >/dev/null
systemctl is-active --quiet chasselfi nginx dnsmasq nftables opennds
runuser -u chasselfi -- ndsctl status >/dev/null

cat <<EOF

ChasselFi vendo installation passed its service checks.
Admin:  http://$(ip -4 -o addr show dev "$WAN_INTERFACE" | awk 'NR == 1 {split($4,a,"/"); print a[1]}')/admin/
Portal: http://10.0.0.1/  (customer VLAN ${VLAN_ID})

Connect one test client to access VLAN ${VLAN_ID}, forget/rejoin WiFi, and open
http://neverssl.com. It must see the branded ChasselFi voucher page and must
not reach the internet until a valid voucher is accepted.
EOF
