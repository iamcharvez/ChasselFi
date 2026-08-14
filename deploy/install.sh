#!/usr/bin/env bash
set -Eeuo pipefail

# Debian places groupadd/useradd/systemctl under administrative sbin paths.
# Some minimal root shells omit those paths, so normalize PATH before any
# command discovery or account/service setup.
export PATH="/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin:${PATH:-}"

APP_NAME="chasselfi"
APP_USER="chasselfi"
APP_GROUP="chasselfi"
PREFIX="/usr/local"
ETC_DIR="/etc/chasselfi"
STATE_DIR="/var/lib/chasselfi"
WITH_NGINX=0
SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_DIR="$(cd -- "${SCRIPT_DIR}/.." && pwd)"

usage() {
  cat <<'EOF'
Usage: sudo ./deploy/install.sh [--with-nginx]

Environment:
  CHASSELFI_ADMIN_USER      admin username (default: admin)
  CHASSELFI_ADMIN_PASSWORD  admin password; generated when omitted
  CHASSELFI_FAS_KEY         openNDS FAS shared key; generated when omitted
  CHASSELFI_COIN_NODE_KEY   ESP32/Arduino/Orange Pi shared key; generated when omitted
EOF
}

for argument in "$@"; do
  case "$argument" in
    --with-nginx) WITH_NGINX=1 ;;
    --help) usage; exit 0 ;;
    *) echo "Unknown option: $argument" >&2; usage >&2; exit 2 ;;
  esac
done

if [[ "${EUID}" -ne 0 ]]; then
  echo "Run this installer as root: sudo $0 $*" >&2
  exit 1
fi
if ! command -v systemctl >/dev/null 2>&1; then
  echo "systemd is required. Use the manual guide for another init system." >&2
  exit 1
fi
if ! command -v cargo >/dev/null 2>&1; then
  echo "Rust/Cargo is required. Install Rust from https://rustup.rs first." >&2
  exit 1
fi
for required_command in groupadd useradd usermod getent install; do
  if ! command -v "${required_command}" >/dev/null 2>&1; then
    echo "Required command '${required_command}' was not found." >&2
    echo "This installer targets Debian/Ubuntu with systemd. On Debian/Ubuntu install it with:" >&2
    echo "  apt-get update && apt-get install -y passwd" >&2
    echo "On Alpine or other non-systemd systems, use the manual guide or a supported Debian/Ubuntu host." >&2
    exit 1
  fi
done

missing_runtime_packages=()
command -v sqlite3 >/dev/null 2>&1 || missing_runtime_packages+=(sqlite3)
command -v openssl >/dev/null 2>&1 || missing_runtime_packages+=(openssl)
if [[ ${#missing_runtime_packages[@]} -gt 0 ]]; then
  if command -v apt-get >/dev/null 2>&1; then
    apt-get update
    DEBIAN_FRONTEND=noninteractive apt-get install -y "${missing_runtime_packages[@]}"
  else
    echo "Missing required recovery tools: ${missing_runtime_packages[*]}" >&2
    exit 1
  fi
fi

if ! getent group "${APP_GROUP}" >/dev/null; then
  groupadd --system "${APP_GROUP}"
fi
if ! id -u "${APP_USER}" >/dev/null 2>&1; then
  useradd --system --gid "${APP_GROUP}" --home-dir "${STATE_DIR}" --create-home --shell /usr/sbin/nologin "${APP_USER}"
fi
# Repair accounts left by early installer versions that created the service
# user before the dedicated group existed. Systemd sets Group= explicitly,
# but maintenance checks using runuser rely on the account's group database.
if ! id -nG "${APP_USER}" | tr ' ' '\n' | grep -Fxq "${APP_GROUP}"; then
  usermod --append --groups "${APP_GROUP}" "${APP_USER}"
fi

echo "Building ${APP_NAME}..."
cargo build --release --manifest-path "${PROJECT_DIR}/Cargo.toml"
install -Dm755 "${PROJECT_DIR}/target/release/${APP_NAME}" "${PREFIX}/bin/${APP_NAME}"
install -d -o "${APP_USER}" -g "${APP_GROUP}" -m0750 "${STATE_DIR}"
install -d -o root -g "${APP_GROUP}" -m0750 "${ETC_DIR}"
install -d -o root -g root -m0700 /var/backups/chasselfi

# The systemd unit runs from STATE_DIR, so install the dashboard and portal
# assets there instead of relying on the source checkout remaining in place.
while IFS= read -r -d '' web_file; do
  relative_file="${web_file#"${PROJECT_DIR}/web/"}"
  install -D -o root -g "${APP_GROUP}" -m0644 \
    "${web_file}" "${STATE_DIR}/web/${relative_file}"
done < <(find "${PROJECT_DIR}/web" -type f -print0)

if [[ ! -f "${ETC_DIR}/config.json" ]]; then
  install -o root -g "${APP_GROUP}" -m0640 \
    "${PROJECT_DIR}/deploy/chasselfi-config.json.example" "${ETC_DIR}/config.json"
fi

admin_user="${CHASSELFI_ADMIN_USER:-admin}"
env_file="${ETC_DIR}/chasselfi.env"
if [[ -z "${CHASSELFI_ADMIN_PASSWORD:-}" && ! -f "${env_file}" ]]; then
  if command -v openssl >/dev/null 2>&1; then
    generated_password="$(openssl rand -hex 18)"
  else
    generated_password="$(date +%s)-$(od -An -N16 -tx1 /dev/urandom | tr -d ' \n')"
  fi
  CHASSELFI_ADMIN_PASSWORD="${generated_password}"
  echo "Generated administrator credentials (save them before closing this terminal)."
  echo "  username: ${admin_user}"
  echo "  password: ${CHASSELFI_ADMIN_PASSWORD}"
fi

if [[ -z "${CHASSELFI_FAS_KEY:-}" ]]; then
  if [[ -f "${env_file}" ]] && grep -q '^CHASSELFI_FAS_KEY=' "${env_file}" 2>/dev/null; then
    CHASSELFI_FAS_KEY="$(sed -n 's/^CHASSELFI_FAS_KEY=//p' "${env_file}" | sed 's/^\x27//;s/\x27$//;s/^"//;s/"$//' | head -n1)"
  elif command -v openssl >/dev/null 2>&1; then
    CHASSELFI_FAS_KEY="$(openssl rand -hex 32)"
  else
    CHASSELFI_FAS_KEY="$(date +%s)-$(od -An -N24 -tx1 /dev/urandom | tr -d ' \n')"
  fi
fi

generated_coin_node_key=0
if [[ -z "${CHASSELFI_COIN_NODE_KEY:-}" ]]; then
  if [[ -f "${env_file}" ]] && grep -q '^CHASSELFI_COIN_NODE_KEY=' "${env_file}" 2>/dev/null; then
    CHASSELFI_COIN_NODE_KEY="$(sed -n 's/^CHASSELFI_COIN_NODE_KEY=//p' "${env_file}" | sed 's/^\x27//;s/\x27$//;s/^"//;s/"$//' | head -n1)"
  elif command -v openssl >/dev/null 2>&1; then
    CHASSELFI_COIN_NODE_KEY="$(openssl rand -hex 32)"
    generated_coin_node_key=1
  else
    CHASSELFI_COIN_NODE_KEY="$(date +%s)-$(od -An -N24 -tx1 /dev/urandom | tr -d ' \n')"
    generated_coin_node_key=1
  fi
fi

if [[ -n "${CHASSELFI_ADMIN_PASSWORD:-}" ]]; then
  umask 077
  printf 'CHASSELFI_ADMIN_USER=%q\nCHASSELFI_ADMIN_PASSWORD=%q\nCHASSELFI_FAS_KEY=%q\nCHASSELFI_COIN_NODE_KEY=%q\nCHASSELFI_COIN_SOCKET=%q\nCHASSELFI_SECURE_COOKIES=%q\n' \
    "${admin_user}" "${CHASSELFI_ADMIN_PASSWORD}" "${CHASSELFI_FAS_KEY:-}" "${CHASSELFI_COIN_NODE_KEY:-}" "/run/chasselfi/coin.sock" "${CHASSELFI_SECURE_COOKIES:-0}" >"${env_file}"
  chown root:"${APP_GROUP}" "${env_file}"
  chmod 0640 "${env_file}"
fi

if [[ ! -f "${env_file}" ]] || ! grep -q '^CHASSELFI_COIN_NODE_KEY=' "${env_file}" 2>/dev/null; then
  umask 077
  printf 'CHASSELFI_COIN_NODE_KEY=%q\nCHASSELFI_COIN_SOCKET=%q\n' \
    "${CHASSELFI_COIN_NODE_KEY}" "/run/chasselfi/coin.sock" >>"${env_file}"
  chown root:"${APP_GROUP}" "${env_file}"
  chmod 0640 "${env_file}"
fi

# This only enables the reviewed privileged helper. No qdisc is changed until
# an administrator explicitly confirms Apply in the Network page.
if [[ ! -f "${env_file}" ]] || ! grep -q '^CHASSELFI_LIVE_ROUTER=' "${env_file}" 2>/dev/null; then
  umask 077
  printf 'CHASSELFI_LIVE_ROUTER=%q\n' "${CHASSELFI_LIVE_ROUTER:-1}" >>"${env_file}"
  chown root:"${APP_GROUP}" "${env_file}"
  chmod 0640 "${env_file}"
fi

if [[ "${generated_coin_node_key}" -eq 1 ]]; then
  echo "Generated network coin-node key (save it in the ESP32/Arduino/Orange Pi firmware)."
  echo "  coin node key: ${CHASSELFI_COIN_NODE_KEY}"
fi

if [[ -n "${CHASSELFI_FAS_KEY:-}" ]]; then
  if [[ ! -f "${env_file}" ]] || ! grep -q '^CHASSELFI_FAS_KEY=' "${env_file}" 2>/dev/null; then
  umask 077
  printf 'CHASSELFI_FAS_KEY=%q\n' "${CHASSELFI_FAS_KEY}" >>"${env_file}"
  chown root:"${APP_GROUP}" "${env_file}"
  chmod 0640 "${env_file}"
  fi
fi

install -o root -g root -m0644 \
  "${PROJECT_DIR}/deploy/chasselfi.service" \
  "/etc/systemd/system/${APP_NAME}.service"
install -D -o root -g root -m0755 \
  "${PROJECT_DIR}/deploy/router/apply-site-blocks.sh" \
  /usr/local/libexec/chasselfi-apply-site-blocks
install -D -o root -g root -m0755 \
  "${PROJECT_DIR}/deploy/router/apply-shaping.sh" \
  /usr/local/libexec/chasselfi-apply-shaping
install -D -o root -g "${APP_GROUP}" -m0750 \
  "${PROJECT_DIR}/deploy/router/chasselfi-coin-pulse.sh" \
  /usr/local/libexec/chasselfi-coin-pulse
install -D -o root -g root -m0750 \
  "${PROJECT_DIR}/deploy/chasselfi-backup.sh" \
  /usr/local/libexec/chasselfi-backup
install -D -o root -g root -m0750 \
  "${PROJECT_DIR}/deploy/chasselfi-restore.sh" \
  /usr/local/libexec/chasselfi-restore
if [[ ! -f "${ETC_DIR}/backup.key" ]]; then
  umask 077
  openssl rand -hex 32 >"${ETC_DIR}/backup.key"
  chown root:root "${ETC_DIR}/backup.key"
  chmod 0600 "${ETC_DIR}/backup.key"
fi
for system_unit in \
  chasselfi-reboot.path chasselfi-reboot.service \
  chasselfi-shutdown.path chasselfi-shutdown.service \
  chasselfi-site-blocks.path chasselfi-site-blocks.service \
  chasselfi-shaping.path chasselfi-shaping.service \
  chasselfi-backup.service chasselfi-backup.timer; do
  install -o root -g root -m0644 \
    "${PROJECT_DIR}/deploy/${system_unit}" "/etc/systemd/system/${system_unit}"
done

if [[ "${WITH_NGINX}" -eq 1 ]]; then
  if ! command -v nginx >/dev/null 2>&1; then
    if command -v apt-get >/dev/null 2>&1; then
      apt-get update
      DEBIAN_FRONTEND=noninteractive apt-get install -y nginx
    else
      echo "Nginx was requested but is not installed; install nginx and rerun with --with-nginx." >&2
    fi
  fi
  if command -v nginx >/dev/null 2>&1; then
    # Ubuntu/Debian's welcome server otherwise wins the `_` server name and
    # hides ChasselFi behind the default Nginx page.
    disabled_dir="/etc/nginx/chasselfi-disabled"
    install -d -m0755 "${disabled_dir}"
    for default_site in /etc/nginx/sites-enabled/default /etc/nginx/sites-enabled/default.disabled; do
      if [[ -e "${default_site}" || -L "${default_site}" ]]; then
        install_name="${disabled_dir}/$(basename "${default_site}").$(date +%s)"
        mv -- "${default_site}" "${install_name}"
      fi
    done
    install -d -m0755 /etc/nginx/conf.d
    install -o root -g root -m0644 \
      "${PROJECT_DIR}/deploy/nginx/chasselfi.conf" \
      "/etc/nginx/conf.d/chasselfi.conf"
    nginx -t
    systemctl enable --now nginx
    systemctl reload nginx
  else
    echo "Continuing without Nginx; ChasselFi is still installed on 127.0.0.1:8080." >&2
  fi
fi

systemctl daemon-reload
systemctl enable --now "${APP_NAME}.service"
systemctl enable --now chasselfi-reboot.path chasselfi-shutdown.path chasselfi-site-blocks.path chasselfi-shaping.path
systemctl enable --now chasselfi-backup.timer

if command -v curl >/dev/null 2>&1; then
  for attempt in 1 2 3 4 5; do
    if curl --fail --silent --show-error http://127.0.0.1:8080/api/health >/dev/null; then
      echo "${APP_NAME} is healthy on 127.0.0.1:8080."
      break
    fi
    sleep 1
  done
fi

cat <<EOF

Installation complete.
Binary:  ${PREFIX}/bin/${APP_NAME}
Config:  ${ETC_DIR}/config.json
State:   ${STATE_DIR}
Service: systemctl status ${APP_NAME}

Review deploy/router/README.md and replace the interface placeholders before
connecting clients. This installer does not change WAN/LAN networking.
EOF
