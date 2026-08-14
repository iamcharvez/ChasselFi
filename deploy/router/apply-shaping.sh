#!/usr/bin/env bash
set -Eeuo pipefail

export PATH="/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin:${PATH:-}"
request=/run/chasselfi/shaping.request
result=/run/chasselfi/shaping.result
[[ "$EUID" -eq 0 ]] || exit 1
[[ -f "$request" ]] || exit 0

lan= wan= download= upload=
while IFS='=' read -r key value; do
    case "$key" in
        lan) lan="$value" ;;
        wan) wan="$value" ;;
        download) download="$value" ;;
        upload) upload="$value" ;;
        *) printf 'error=unknown request field\n' >"$result"; rm -f "$request"; exit 1 ;;
    esac
done <"$request"

valid_interface() { [[ "$1" =~ ^[A-Za-z0-9_.-]{1,15}$ ]] && ip link show dev "$1" >/dev/null 2>&1; }
valid_rate() { [[ "$1" =~ ^[0-9]{1,5}$ ]] && (( 10#$1 >= 1 && 10#$1 <= 10000 )); }
if ! valid_interface "$lan" || ! valid_rate "$download" || ! valid_rate "$upload" \
    || { [[ -n "$wan" ]] && ! valid_interface "$wan"; }; then
    printf 'error=invalid shaping request\n' >"$result"
    rm -f "$request"
    exit 1
fi

modprobe sch_cake 2>/dev/null || true
if ! tc qdisc replace dev "$lan" root cake bandwidth "${download}Mbit" \
    besteffort dual-dsthost nat wash 2>"${result}.stderr"; then
    printf 'error=LAN CAKE failed: %s\n' "$(tr '\n' ' ' <"${result}.stderr")" >"$result"
    rm -f "$request" "${result}.stderr"
    exit 1
fi
if [[ -n "$wan" ]] && ! tc qdisc replace dev "$wan" root cake bandwidth "${upload}Mbit" \
    besteffort dual-srchost nat wash ack-filter 2>"${result}.stderr"; then
    printf 'error=WAN CAKE failed: %s\n' "$(tr '\n' ' ' <"${result}.stderr")" >"$result"
    rm -f "$request" "${result}.stderr"
    exit 1
fi
if ! tc qdisc show dev "$lan" | grep -qw cake \
    || { [[ -n "$wan" ]] && ! tc qdisc show dev "$wan" | grep -qw cake; }; then
    printf 'error=CAKE command returned success but verification failed\n' >"$result"
    rm -f "$request" "${result}.stderr"
    exit 1
fi
printf 'ok=CAKE verified: %s Mbps aggregate download on %s%s; per-device fairness enabled\n' \
    "$download" "$lan" "${wan:+ and $upload Mbps aggregate upload on $wan}" >"$result"
rm -f "$request" "${result}.stderr"
chown root:chasselfi "$result"
chmod 0660 "$result"
