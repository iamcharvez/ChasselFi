#!/usr/bin/env bash
set -Eeuo pipefail

export PATH="/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin:${PATH:-}"
SOCKET=""

for _ in $(seq 1 300); do
    for candidate in /tmp/ndsctl.sock /run/ndsctl.sock /run/opennds/ndsctl.sock; do
        if [[ -S "$candidate" ]]; then
            SOCKET="$candidate"
            break 2
        fi
    done
    sleep 0.1
done

if [[ -n "$SOCKET" ]]; then
    if getent group chasselfi >/dev/null 2>&1; then
        # ndsctl serializes every request with a lock file in the same tmpfs
        # directory as its control socket. The first root invocation creates
        # that file as 0600, which otherwise prevents the unprivileged
        # ChasselFi service from using ndsctl even when the socket is 0660.
        LOCKFILE="$(dirname -- "$SOCKET")/ndsctl.lock"
        touch "$LOCKFILE"
        chgrp chasselfi "$LOCKFILE"
        chmod 0660 "$LOCKFILE"
        chgrp chasselfi "$SOCKET"
        chmod 0660 "$SOCKET"
        exit 0
    fi
    echo "ChasselFi group does not exist" >&2
    exit 1
fi

echo "openNDS control socket did not appear in /tmp or /run after 30 seconds" >&2
exit 1
