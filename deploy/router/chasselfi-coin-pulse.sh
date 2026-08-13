#!/usr/bin/env bash
set -Eeuo pipefail

count="${1:-1}"
socket="${CHASSELFI_COIN_SOCKET:-/run/chasselfi/coin.sock}"
[[ "$count" =~ ^[0-9]+$ ]] && (( count >= 1 && count <= 100 )) || {
    echo "Usage: $0 [pulse-count 1-100]" >&2
    exit 2
}
[[ "$socket" == /run/chasselfi/* ]] || {
    echo "Refusing coin socket outside /run/chasselfi" >&2
    exit 2
}

python3 - "$socket" "$count" <<'PY'
import socket
import sys

destination, count = sys.argv[1], sys.argv[2]
client = socket.socket(socket.AF_UNIX, socket.SOCK_DGRAM)
client.sendto(f"PULSE {count}".encode("ascii"), destination)
PY
