#!/usr/bin/env bash
set -Eeuo pipefail

export PATH="/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin:${PATH:-}"
STATE_DIR="${CHASSELFI_STATE_DIR:-/var/lib/chasselfi}"
CONFIG_DIR="${CHASSELFI_CONFIG_DIR:-/etc/chasselfi}"
BACKUP_DIR="${CHASSELFI_BACKUP_DIR:-/var/backups/chasselfi}"
RETENTION_DAYS="${CHASSELFI_BACKUP_RETENTION_DAYS:-}"
KEY_FILE="${CHASSELFI_BACKUP_KEY_FILE:-/etc/chasselfi/backup.key}"
DATABASE="${STATE_DIR}/chasselfi.sqlite3"

[[ "$EUID" -eq 0 ]] || { echo "Run as root." >&2; exit 1; }
[[ -r "$DATABASE" ]] || { echo "Database not found: $DATABASE" >&2; exit 1; }
[[ -r "$KEY_FILE" ]] || { echo "Backup key not found: $KEY_FILE" >&2; exit 1; }
command -v sqlite3 >/dev/null || { echo "sqlite3 is required." >&2; exit 1; }
command -v openssl >/dev/null || { echo "openssl is required." >&2; exit 1; }

# The dashboard owns this policy. An explicit service environment value still
# wins, while older databases and SQLite builds without JSON support safely
# fall back to 30 days.
if [[ -z "$RETENTION_DAYS" ]]; then
    RETENTION_DAYS="$(sqlite3 "$DATABASE" \
        "SELECT COALESCE(json_extract(payload, '$.settings.backupRetentionDays'), 30) FROM app_state WHERE id=1;" \
        2>/dev/null || true)"
fi
[[ "$RETENTION_DAYS" =~ ^[0-9]{1,4}$ ]] && (( 10#$RETENTION_DAYS >= 1 && 10#$RETENTION_DAYS <= 3650 )) \
    || RETENTION_DAYS=30

install -d -o root -g root -m0700 "$BACKUP_DIR"
work_dir="$(mktemp -d -p /var/tmp chasselfi-backup.XXXXXX)"
trap 'rm -rf -- "$work_dir"' EXIT
stamp="$(date -u +%Y%m%dT%H%M%SZ)"
archive="${BACKUP_DIR}/chasselfi-${stamp}.tar.gz.enc"

sqlite3 "$DATABASE" ".timeout 10000" ".backup '${work_dir}/chasselfi.sqlite3'"
sqlite3 "${work_dir}/chasselfi.sqlite3" "PRAGMA integrity_check;" | grep -Fxq ok
install -d -m0700 "${work_dir}/config"
for file in config.json chasselfi.env; do
    [[ -f "${CONFIG_DIR}/${file}" ]] && install -m0600 "${CONFIG_DIR}/${file}" "${work_dir}/config/${file}"
done
printf 'created_at=%s\nhost=%s\n' "$(date -u --iso-8601=seconds)" "$(hostname)" >"${work_dir}/MANIFEST"
(cd "$work_dir" && sha256sum chasselfi.sqlite3 config/* 2>/dev/null >>MANIFEST || true)
tar -C "$work_dir" -czf - chasselfi.sqlite3 config MANIFEST | \
    openssl enc -aes-256-cbc -pbkdf2 -salt -pass "file:${KEY_FILE}" -out "$archive"
chmod 0600 "$archive"
sha256sum "$archive" >"${archive}.sha256"
chmod 0600 "${archive}.sha256"
find "$BACKUP_DIR" -maxdepth 1 -type f -name 'chasselfi-*.tar.gz.enc*' -mtime "+${RETENTION_DAYS}" -delete
echo "$archive"
