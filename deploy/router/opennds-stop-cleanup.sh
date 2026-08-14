#!/usr/bin/env bash
set -Eeuo pipefail

export PATH="/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin:${PATH:-}"
CONFIG_READER=/usr/lib/opennds/libopennds.sh

heartbeat_alive() {
    [[ -r "$CONFIG_READER" ]] \
        && /bin/bash "$CONFIG_READER" check_heartbeat >/dev/null 2>&1
}

# openNDS forks. Some distribution units report the parent stopped while the
# child heartbeat and ndsctl socket are still alive. Block the subsequent
# systemd start until the old instance genuinely releases the data plane.
for _ in $(seq 1 400); do
    if ! pgrep -x opennds >/dev/null 2>&1 && ! heartbeat_alive; then
        rm -f /tmp/ndsctl.sock /tmp/ndsctl.lock \
            /run/ndsctl.sock /run/ndsctl.lock \
            /run/opennds/ndsctl.sock /run/opennds/ndsctl.lock
        exit 0
    fi
    sleep 0.1
done

echo "openNDS process or heartbeat did not stop within 40 seconds" >&2
pgrep -a -x opennds >&2 2>/dev/null || true
exit 1
