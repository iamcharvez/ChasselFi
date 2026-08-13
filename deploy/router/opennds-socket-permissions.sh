#!/usr/bin/env bash
set -Eeuo pipefail

export PATH="/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin:${PATH:-}"
SOCKET="/tmp/ndsctl.sock"

for _ in $(seq 1 60); do
    if [[ -S "$SOCKET" ]]; then
        chgrp chasselfi "$SOCKET"
        chmod 0660 "$SOCKET"
        exit 0
    fi
    sleep 0.1
done

echo "openNDS control socket did not appear: $SOCKET" >&2
exit 1
