#!/usr/bin/env bash
set -Eeuo pipefail

export PATH="/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin:${PATH:-}"
REQUEST=/run/chasselfi/site-blocks.request
DESTINATION=/etc/dnsmasq.d/chasselfi-blocked.conf

[[ -f "$REQUEST" ]] || exit 0
command -v dnsmasq >/dev/null 2>&1 || { echo "dnsmasq is not installed" >&2; exit 1; }

temporary="$(mktemp /run/chasselfi/site-blocks.XXXXXX)"
trap 'rm -f -- "$temporary"' EXIT
{
    echo "# Managed by ChasselFi. Manual changes will be replaced."
    while IFS= read -r hostname; do
        [[ -n "$hostname" ]] || continue
        [[ ${#hostname} -le 253 ]] || { echo "invalid hostname in request" >&2; exit 1; }
        [[ "$hostname" =~ ^[a-z0-9]([a-z0-9.-]*[a-z0-9])?$ ]] \
            || { echo "invalid hostname in request" >&2; exit 1; }
        [[ "$hostname" != *..* ]] || { echo "invalid hostname in request" >&2; exit 1; }
        printf 'address=/%s/0.0.0.0\n' "$hostname"
        printf 'address=/%s/::\n' "$hostname"
    done <"$REQUEST"
} >"$temporary"

install -D -o root -g root -m0644 "$temporary" "$DESTINATION"
dnsmasq --test
systemctl reload-or-restart dnsmasq
rm -f -- "$REQUEST"

