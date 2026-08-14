#!/usr/bin/env bash
set -Eeuo pipefail

export PATH="/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin:${PATH:-}"
SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_DIR="$(cd -- "${SCRIPT_DIR}/../.." && pwd)"
WAN_INTERFACE="${CHASSELFI_WAN:-}"
VLAN_ID="${CHASSELFI_VLAN_ID:-799}"
WAN_DOWNLOAD_MBPS="${CHASSELFI_WAN_DOWNLOAD_MBPS:-142}"
WAN_UPLOAD_MBPS="${CHASSELFI_WAN_UPLOAD_MBPS:-142}"
ASSUME_YES=0

usage() {
    cat <<'EOF'
Usage: sudo bash deploy/router/install-vendo-vlan799.sh [options]

Options:
  --wan IFACE       Untagged WAN interface (default: default-route interface)
  --vlan-id ID      Tagged customer VLAN (default: 799)
  --yes             Skip the final confirmation
  -h, --help        Show this help

For a new installation, set CHASSELFI_ADMIN_PASSWORD before running or the
script securely prompts for it. Existing administrator credentials are kept.
The switch port facing the server must carry the customer VLAN tagged.
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
[[ "$WAN_DOWNLOAD_MBPS" =~ ^[0-9]+$ ]] && (( WAN_DOWNLOAD_MBPS >= 1 && WAN_DOWNLOAD_MBPS <= 10000 )) \
    || die "CHASSELFI_WAN_DOWNLOAD_MBPS must be between 1 and 10000"
[[ "$WAN_UPLOAD_MBPS" =~ ^[0-9]+$ ]] && (( WAN_UPLOAD_MBPS >= 1 && WAN_UPLOAD_MBPS <= 10000 )) \
    || die "CHASSELFI_WAN_UPLOAD_MBPS must be between 1 and 10000"

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

existing_password=0
if [[ -f /etc/chasselfi/chasselfi.env ]] \
    && grep -q '^CHASSELFI_ADMIN_PASSWORD=' /etc/chasselfi/chasselfi.env; then
    existing_password=1
fi

if [[ -z "${CHASSELFI_ADMIN_PASSWORD:-}" && "$existing_password" -ne 1 ]]; then
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
  CAKE ceilings:   ${WAN_DOWNLOAD_MBPS}/${WAN_UPLOAD_MBPS} Mbps down/up
EOF
if [[ "$ASSUME_YES" -ne 1 ]]; then
    read -r -p "Install and activate this router configuration? [y/N] " answer
    [[ "$answer" =~ ^[Yy]$ ]] || { echo "Cancelled."; exit 0; }
fi

chmod +x "$PROJECT_DIR/deploy/install.sh" "$SCRIPT_DIR"/*.sh
"$PROJECT_DIR/deploy/install.sh" --with-nginx
"$SCRIPT_DIR/setup-vlan799.sh" --wan "$WAN_INTERFACE" --vlan-id "$VLAN_ID" --yes

# The production installer also writes CHASSELFI_HARDWARE_MODE=linux into the
# service environment. Keep the JSON explicit for operator readability.
sed -i 's/"hardware_mode"[[:space:]]*:[[:space:]]*"simulated"/"hardware_mode": "linux"/' \
    /etc/chasselfi/config.json
systemctl restart chasselfi nginx

"$SCRIPT_DIR/setup-opennds.sh" --lan "${WAN_INTERFACE}.${VLAN_ID}" --yes

# Apply and verify the aggregate CAKE ceilings during a full production
# install. The dashboard can change these values later through the same
# restricted helper. A 150 Mbps measured line normally uses about 142 Mbps
# here (95 percent) so CAKE, rather than the ISP modem, owns the queue.
install -d -o chasselfi -g chasselfi -m0750 /run/chasselfi
systemctl stop chasselfi-shaping.path
cat >/run/chasselfi/shaping.request <<EOF
lan=${WAN_INTERFACE}.${VLAN_ID}
wan=${WAN_INTERFACE}
download=${WAN_DOWNLOAD_MBPS}
upload=${WAN_UPLOAD_MBPS}
EOF
chown chasselfi:chasselfi /run/chasselfi/shaping.request
chmod 0660 /run/chasselfi/shaping.request
if ! /usr/local/libexec/chasselfi-apply-shaping; then
    systemctl enable --now chasselfi-shaping.path
    [[ -f /run/chasselfi/shaping.result ]] && cat /run/chasselfi/shaping.result >&2
    die "CAKE shaping helper failed"
fi
systemctl enable --now chasselfi-shaping.path
grep -q '^ok=' /run/chasselfi/shaping.result \
    || { cat /run/chasselfi/shaping.result >&2; die "CAKE shaping did not pass verification"; }
cat /run/chasselfi/shaping.result
tc qdisc show dev "${WAN_INTERFACE}.${VLAN_ID}" | grep -qw cake \
    || die "CAKE is missing from the customer VLAN after installation"
tc qdisc show dev "${WAN_INTERFACE}" | grep -qw cake \
    || die "CAKE is missing from the WAN after installation"

curl --fail --silent --show-error http://127.0.0.1:8080/api/health >/dev/null
curl --fail --silent --show-error http://10.0.0.1/portal.html >/dev/null
curl --fail --silent --show-error http://10.0.0.1:2080/styles.css >/dev/null
systemctl is-active --quiet chasselfi nginx dnsmasq nftables opennds
runuser -u chasselfi -- ndsctl status >/dev/null
grep -q '"hardwareMode":"linux"' <(curl --fail --silent --show-error http://127.0.0.1:8080/api/health)

cat <<EOF

ChasselFi vendo installation passed its service checks.
Admin:  http://$(ip -4 -o addr show dev "$WAN_INTERFACE" | awk 'NR == 1 {split($4,a,"/"); print a[1]}')/admin/
Portal: http://10.0.0.1/  (customer VLAN ${VLAN_ID})

Connect one test client to access VLAN ${VLAN_ID}, forget/rejoin WiFi, and open
http://neverssl.com. It must see the branded ChasselFi voucher page and must
not reach the internet until a valid voucher is accepted.
EOF
