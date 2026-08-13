#!/usr/bin/env bash
set -Eeuo pipefail

# Configure a routed ChasselFi LAN on VLAN 4001 using the same physical NIC
# as the WAN. The WAN remains untagged on the parent interface; the LAN is
# created as <wan>.<vlan-id> with address 10.0.0.1/20.

VLAN_ID=4001
LAN_IP=10.0.0.1
LAN_CIDR=10.0.0.1/20
LAN_NETWORK=10.0.0.0/20
DHCP_START=10.0.0.100
DHCP_END=10.0.15.250
WAN_INTERFACE="${CHASSELFI_WAN:-}"
ASSUME_YES=0

usage() {
    cat <<'EOF'
Usage: setup-vlan4001.sh [options]

Options:
  --wan IFACE       WAN parent interface (default: interface with default route)
  --vlan-id ID      VLAN ID (default: 4001)
  --yes             Apply without an interactive confirmation
  -h, --help        Show this help

The LAN is configured as 10.0.0.1/20 and DHCP as 10.0.0.100-10.0.15.250.
Run this script as root from a local console or out-of-band session.
EOF
}

die() {
    echo "ERROR: $*" >&2
    exit 1
}

while [[ $# -gt 0 ]]; do
    case "$1" in
        --wan)
            [[ $# -ge 2 ]] || die "--wan requires an interface name"
            WAN_INTERFACE="$2"
            shift 2
            ;;
        --vlan-id)
            [[ $# -ge 2 ]] || die "--vlan-id requires a number"
            VLAN_ID="$2"
            shift 2
            ;;
        --yes)
            ASSUME_YES=1
            shift
            ;;
        -h|--help)
            usage
            exit 0
            ;;
        *)
            die "Unknown option: $1"
            ;;
    esac
done

[[ "${EUID}" -eq 0 ]] || die "run as root"
export PATH="/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin:${PATH:-}"

for command in ip awk grep sed install systemctl; do
    command -v "$command" >/dev/null 2>&1 || die "required command not found: $command"
done

if [[ -z "$WAN_INTERFACE" ]]; then
    WAN_INTERFACE="$(ip -4 route show default | awk 'NR == 1 {print $5}')"
fi
[[ -n "$WAN_INTERFACE" ]] || die "could not detect a default-route WAN; pass --wan IFACE"
ip link show "$WAN_INTERFACE" >/dev/null 2>&1 || die "WAN interface does not exist: $WAN_INTERFACE"

[[ "$VLAN_ID" =~ ^[0-9]+$ ]] || die "VLAN ID must be numeric"
(( VLAN_ID >= 1 && VLAN_ID <= 4094 )) || die "VLAN ID must be between 1 and 4094"

VLAN_INTERFACE="${WAN_INTERFACE}.${VLAN_ID}"
BACKUP_DIR="/var/backups/chasselfi-router/$(date +%Y%m%d-%H%M%S)"
NETWORK_FILE="/etc/network/interfaces.d/chasselfi-vlan${VLAN_ID}"
DNSMASQ_FILE="/etc/dnsmasq.d/chasselfi.conf"
NFT_FILE="/etc/nftables.d/chasselfi.nft"
SYSCTL_FILE="/etc/sysctl.d/99-chasselfi-router.conf"
MODULE_FILE="/etc/modules-load.d/8021q.conf"

cat <<EOF
ChasselFi VLAN router plan
  WAN parent:  ${WAN_INTERFACE}
  LAN VLAN:    ${VLAN_INTERFACE} (802.1Q VLAN ${VLAN_ID})
  LAN address: ${LAN_CIDR}
  DHCP range:  ${DHCP_START} - ${DHCP_END}
  NAT source:  ${LAN_NETWORK} -> ${WAN_INTERFACE}
EOF

if [[ "$ASSUME_YES" -ne 1 ]]; then
    read -r -p "Apply this network configuration? [y/N] " answer
    [[ "$answer" =~ ^[Yy]$ ]] || { echo "Cancelled."; exit 0; }
fi

backup_if_present() {
    local file="$1"
    [[ -e "$file" ]] || return 0
    mkdir -p "$BACKUP_DIR"
    cp -a -- "$file" "$BACKUP_DIR/$(basename "$file")"
}

write_if_changed() {
    local file="$1"
    local mode="$2"
    local tmp
    tmp="$(mktemp)"
    cat >"$tmp"
    if [[ -f "$file" ]] && cmp -s "$tmp" "$file"; then
        rm -f -- "$tmp"
        return 0
    fi
    backup_if_present "$file"
    install -D -m "$mode" "$tmp" "$file"
    rm -f -- "$tmp"
}

missing_packages=()
if command -v dpkg-query >/dev/null 2>&1; then
    for package in vlan dnsmasq nftables kmod; do
        if ! dpkg-query -W -f='${Status}' "$package" 2>/dev/null | grep -q 'install ok installed'; then
            missing_packages+=("$package")
        fi
    done
fi
if [[ "${#missing_packages[@]}" -gt 0 ]]; then
    command -v apt-get >/dev/null 2>&1 || die "missing packages (${missing_packages[*]}) and apt-get is unavailable"
    apt-get update
    apt-get install -y "${missing_packages[@]}"
fi

modprobe 8021q
write_if_changed "$MODULE_FILE" 0644 <<'EOF'
8021q
EOF

write_if_changed "$NETWORK_FILE" 0644 <<EOF
auto ${VLAN_INTERFACE}
iface ${VLAN_INTERFACE} inet static
    address ${LAN_IP}
    netmask 255.255.240.0
    vlan-raw-device ${WAN_INTERFACE}
EOF

write_if_changed "$SYSCTL_FILE" 0644 <<'EOF'
net.ipv4.ip_forward=1
EOF

write_if_changed "$DNSMASQ_FILE" 0644 <<EOF
interface=${VLAN_INTERFACE}
bind-interfaces
dhcp-authoritative
dhcp-range=${DHCP_START},${DHCP_END},12h
dhcp-option=3,${LAN_IP}
dhcp-option=6,${LAN_IP}
domain-needed
bogus-priv
server=1.1.1.1
server=8.8.8.8
EOF

mkdir -p /etc/nftables.d
write_if_changed "$NFT_FILE" 0644 <<EOF
table inet chasselfi_filter {
    chain forward {
        type filter hook forward priority filter;
        policy drop;

        ct state established,related accept
        iifname "${VLAN_INTERFACE}" oifname "${WAN_INTERFACE}" accept
    }
}

table ip chasselfi_nat {
    chain postrouting {
        type nat hook postrouting priority srcnat;
        policy accept;

        oifname "${WAN_INTERFACE}" ip saddr ${LAN_NETWORK} masquerade
    }
}
EOF

if [[ ! -f /etc/nftables.conf ]]; then
    write_if_changed /etc/nftables.conf 0644 <<'EOF'
#!/usr/sbin/nft -f
flush ruleset
include "/etc/nftables.d/*.nft"
EOF
elif ! grep -Fqx 'include "/etc/nftables.d/*.nft"' /etc/nftables.conf; then
    backup_if_present /etc/nftables.conf
    printf '\ninclude "/etc/nftables.d/*.nft"\n' >>/etc/nftables.conf
fi

if ip link show "$VLAN_INTERFACE" >/dev/null 2>&1; then
    existing_vlan_id="$(ip -d link show "$VLAN_INTERFACE" | sed -n 's/.*vlan id \([0-9][0-9]*\).*/\1/p' | head -n 1)"
    [[ "$existing_vlan_id" == "$VLAN_ID" ]] || die "${VLAN_INTERFACE} already exists with VLAN ID ${existing_vlan_id:-unknown}"
else
    ip link add link "$WAN_INTERFACE" name "$VLAN_INTERFACE" type vlan id "$VLAN_ID"
fi
ip addr replace "$LAN_CIDR" dev "$VLAN_INTERFACE"
ip link set dev "$VLAN_INTERFACE" up

sysctl --system >/dev/null
dnsmasq --test
nft -c -f /etc/nftables.conf

systemctl enable nftables dnsmasq
systemctl restart nftables
systemctl restart dnsmasq

echo
echo "VLAN router configuration applied successfully."
ip -br addr show dev "$VLAN_INTERFACE"
echo "Backups (if any): ${BACKUP_DIR}"
echo
echo "Connect the switch/client port to tagged VLAN ${VLAN_ID}."
echo "Clients should receive ${DHCP_START}-${DHCP_END} with gateway ${LAN_IP}."
