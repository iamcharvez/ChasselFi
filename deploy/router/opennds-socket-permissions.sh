#!/usr/bin/env bash
set -Eeuo pipefail

export PATH="/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin:${PATH:-}"
SOCKET=""
NDSCTL_ERROR=""

for _ in $(seq 1 300); do
    SOCKET=""
    for candidate in /tmp/ndsctl.sock /run/ndsctl.sock /run/opennds/ndsctl.sock; do
        if [[ -S "$candidate" ]]; then
            SOCKET="$candidate"
            break
        fi
    done

    if [[ -n "$SOCKET" ]] && getent group chasselfi >/dev/null 2>&1; then
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

        # The daemon can create its socket before it begins accepting control
        # requests, and some builds replace it during late initialization.
        # Keep repairing the current inode until the real service account can
        # complete an ndsctl request.
        if NDSCTL_ERROR="$(runuser -u chasselfi -- ndsctl status 2>&1)"; then
            exit 0
        fi
    fi

    sleep 0.1
done

echo "openNDS control socket was not usable by chasselfi after 30 seconds" >&2
id chasselfi >&2 2>/dev/null || true
if [[ -n "$SOCKET" ]]; then
    namei -l "$SOCKET" >&2 2>/dev/null || ls -ld "$(dirname -- "$SOCKET")" "$SOCKET" >&2 2>/dev/null || true
    ls -l "$(dirname -- "$SOCKET")/ndsctl.lock" >&2 2>/dev/null || true
fi
printf '%s\n' "$NDSCTL_ERROR" >&2
exit 1
