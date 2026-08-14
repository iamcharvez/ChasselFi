#!/usr/bin/env bash
set -Eeuo pipefail

export PATH="/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin:${PATH:-}"
backup="${1:-}"
[[ "$EUID" -eq 0 ]] || { echo "Run as root." >&2; exit 1; }
[[ -f "$backup" ]] || { echo "Usage: $0 /var/backups/chasselfi/chasselfi-TIMESTAMP.tar.gz.enc" >&2; exit 2; }
key_file="${CHASSELFI_BACKUP_KEY_FILE:-/etc/chasselfi/backup.key}"
[[ -r "$key_file" ]] || { echo "Backup key not found: $key_file" >&2; exit 1; }
[[ -f "${backup}.sha256" ]] && (cd "$(dirname "$backup")" && sha256sum -c "$(basename "${backup}.sha256")")

work_dir="$(mktemp -d -p /var/tmp chasselfi-restore.XXXXXX)"
service_stopped=0
cleanup() {
    rm -rf -- "$work_dir"
    if [[ "$service_stopped" -eq 1 ]]; then
        systemctl start chasselfi >/dev/null 2>&1 || true
    fi
}
trap cleanup EXIT
archive="${work_dir}/backup.tar.gz"
openssl enc -d -aes-256-cbc -pbkdf2 -pass "file:${key_file}" -in "$backup" -out "$archive"
while IFS= read -r member; do
    case "$member" in
        chasselfi.sqlite3|MANIFEST|config|config/|config/config.json|config/chasselfi.env) ;;
        *) echo "Backup contains an unsafe or unexpected path: $member" >&2; exit 1 ;;
    esac
done < <(tar -tzf "$archive")
tar -C "$work_dir" --no-same-owner --no-same-permissions -xzf "$archive"
[[ -f "${work_dir}/chasselfi.sqlite3" ]] || { echo "Backup has no database." >&2; exit 1; }
sqlite3 "${work_dir}/chasselfi.sqlite3" "PRAGMA integrity_check;" | grep -Fxq ok

read -r -p "Stop ChasselFi and restore this verified backup? [y/N] " answer
[[ "$answer" =~ ^[Yy]$ ]] || { echo "Cancelled."; exit 0; }
systemctl stop chasselfi
service_stopped=1
install -d -m0700 /var/lib/chasselfi/recovery
[[ -f /var/lib/chasselfi/chasselfi.sqlite3 ]] && cp -a /var/lib/chasselfi/chasselfi.sqlite3 "/var/lib/chasselfi/recovery/pre-cli-restore-$(date -u +%Y%m%dT%H%M%SZ).sqlite3"
install -o chasselfi -g chasselfi -m0600 "${work_dir}/chasselfi.sqlite3" /var/lib/chasselfi/chasselfi.sqlite3
for file in config.json chasselfi.env; do
    [[ -f "${work_dir}/config/${file}" ]] && install -o root -g chasselfi -m0640 "${work_dir}/config/${file}" "/etc/chasselfi/${file}"
done
systemctl start chasselfi
systemctl is-active --quiet chasselfi
service_stopped=0
echo "Restore completed and ChasselFi is active."
