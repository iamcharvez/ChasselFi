#!/usr/bin/env bash
set -Eeuo pipefail

# Backward-compatible entry point. The default VLAN is now 799.
exec "$(dirname "$0")/setup-vlan799.sh" "$@"
