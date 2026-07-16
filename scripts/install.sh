#!/usr/bin/env bash
set -Eeuo pipefail

readonly REPOSITORY="${PULSE_REPOSITORY:-onewesong/pulse}"
readonly REQUESTED_VERSION="${VERSION:-latest}"
readonly TARGET="x86_64-unknown-linux-gnu"
readonly ARCHIVE_NAME="pulse-${TARGET}.tar.gz"
readonly INSTALL_ROOT="${PULSE_INSTALL_ROOT:-}"
readonly DOCKER_SOCKET="${PULSE_DOCKER_SOCKET:-/var/run/docker.sock}"
readonly PULSE_USER="pulse"
readonly DOCKER_GROUP="docker"
readonly BINARY_PATH="${INSTALL_ROOT}/usr/local/bin/pulse"
readonly CONFIG_DIR="${INSTALL_ROOT}/etc/pulse"
readonly CONFIG_PATH="${CONFIG_DIR}/config.toml"
readonly SERVICE_PATH="${INSTALL_ROOT}/etc/systemd/system/pulse.service"
readonly DATA_DIR="${INSTALL_ROOT}/var/lib/pulse"

TEMP_DIR=""

log() {
    printf '[pulse] %s\n' "$*"
}

die() {
    printf '[pulse] 错误：%s\n' "$*" >&2
    exit 1
}

cleanup() {
    if [[ -n "${TEMP_DIR}" && -d "${TEMP_DIR}" ]]; then
        rm -rf "${TEMP_DIR}"
    fi
}

trap cleanup EXIT

require_command() {
    command -v "$1" >/dev/null 2>&1 || die "缺少必要命令：$1"
}

check_environment() {
    [[ "$(id -u)" == "0" ]] || die "请以 root 运行，例如：curl ... | sudo bash"
    [[ "$(uname -s)" == "Linux" ]] || die "当前仅支持 Linux"

    case "$(uname -m)" in
        x86_64 | amd64) ;;
        *) die "当前仅支持 x86_64，检测到架构：$(uname -m)" ;;
    esac

    for command_name in curl tar sha256sum awk install find systemctl journalctl useradd usermod getent id chown mktemp mkdir rm; do
        require_command "${command_name}"
    done
    require_command docker

    [[ -S "${DOCKER_SOCKET}" ]] || die "未找到 Docker Socket：${DOCKER_SOCKET}"
    getent group "${DOCKER_GROUP}" >/dev/null 2>&1 || die "未找到 docker 用户组，请先安装并启动 Docker Engine"
}

release_base_url() {
    if [[ -n "${PULSE_RELEASE_BASE_URL:-}" ]]; then
        printf '%s\n' "${PULSE_RELEASE_BASE_URL%/}"
        return
    fi

    if [[ "${REQUESTED_VERSION}" == "latest" ]]; then
        printf 'https://github.com/%s/releases/latest/download\n' "${REPOSITORY}"
        return
    fi

    local version="${REQUESTED_VERSION#v}"
    [[ "${version}" =~ ^[0-9]+\.[0-9]+\.[0-9]+([.-][0-9A-Za-z.-]+)?$ ]] \
        || die "无效的 VERSION：${REQUESTED_VERSION}"
    printf 'https://github.com/%s/releases/download/v%s\n' "${REPOSITORY}" "${version}"
}

download_file() {
    local url="$1"
    local destination="$2"
    local options=(--fail --location --silent --show-error --retry 3)

    if [[ -z "${PULSE_RELEASE_BASE_URL:-}" ]]; then
        options+=(--proto '=https' --tlsv1.2)
    fi
    curl "${options[@]}" --output "${destination}" "${url}"
}

download_and_verify() {
    local base_url="$1"
    local archive_path="${TEMP_DIR}/${ARCHIVE_NAME}"
    local checksum_path="${TEMP_DIR}/SHA256SUMS"
    local selected_checksum="${TEMP_DIR}/archive.sha256"

    log "从 ${base_url} 下载发布包"
    download_file "${base_url}/${ARCHIVE_NAME}" "${archive_path}"
    download_file "${base_url}/SHA256SUMS" "${checksum_path}"

    awk -v name="${ARCHIVE_NAME}" '$2 == name || $2 == "*" name { print; found = 1 } END { if (!found) exit 1 }' \
        "${checksum_path}" >"${selected_checksum}" \
        || die "SHA256SUMS 中缺少 ${ARCHIVE_NAME}"

    if ! (cd "${TEMP_DIR}" && sha256sum --check --status "$(basename "${selected_checksum}")"); then
        die "发布包 SHA-256 校验失败"
    fi
    log "SHA-256 校验通过"
}

extract_package() {
    local extract_dir="${TEMP_DIR}/extract"
    mkdir -p "${extract_dir}"
    tar -xzf "${TEMP_DIR}/${ARCHIVE_NAME}" -C "${extract_dir}"

    PACKAGE_DIR="$(find "${extract_dir}" -mindepth 1 -maxdepth 1 -type d -name 'pulse-*' -print -quit)"
    [[ -n "${PACKAGE_DIR}" ]] || die "发布包目录结构无效"

    for required_file in pulse config/pulse.toml packaging/pulse.service; do
        [[ -f "${PACKAGE_DIR}/${required_file}" ]] || die "发布包缺少文件：${required_file}"
    done
}

ensure_service_user() {
    if ! id -u "${PULSE_USER}" >/dev/null 2>&1; then
        local nologin_shell
        nologin_shell="$(command -v nologin || true)"
        [[ -n "${nologin_shell}" ]] || nologin_shell="/usr/sbin/nologin"
        log "创建 ${PULSE_USER} 系统用户"
        useradd --system --home-dir /var/lib/pulse --shell "${nologin_shell}" "${PULSE_USER}"
    fi
    usermod --append --groups "${DOCKER_GROUP}" "${PULSE_USER}"
}

install_files() {
    log "安装 Pulse 文件"
    install -Dm755 "${PACKAGE_DIR}/pulse" "${BINARY_PATH}"
    install -Dm644 "${PACKAGE_DIR}/packaging/pulse.service" "${SERVICE_PATH}"
    install -d -m755 "${CONFIG_DIR}"
    install -d -m750 "${DATA_DIR}"
    chown "${PULSE_USER}:${PULSE_USER}" "${DATA_DIR}"

    if [[ -e "${CONFIG_PATH}" ]]; then
        log "保留现有配置：${CONFIG_PATH}"
    else
        install -m640 "${PACKAGE_DIR}/config/pulse.toml" "${CONFIG_PATH}"
        chown "root:${PULSE_USER}" "${CONFIG_PATH}"
        log "已安装默认配置：${CONFIG_PATH}"
    fi
}

start_service() {
    log "启用并启动 pulse.service"
    systemctl daemon-reload
    systemctl enable pulse.service >/dev/null

    if ! systemctl restart pulse.service; then
        journalctl -u pulse.service --no-pager -n 50 >&2 || true
        die "pulse.service 启动失败"
    fi
    if ! systemctl is-active --quiet pulse.service; then
        journalctl -u pulse.service --no-pager -n 50 >&2 || true
        die "pulse.service 未处于运行状态"
    fi
}

main() {
    check_environment
    TEMP_DIR="$(mktemp -d)"
    local base_url
    base_url="$(release_base_url)"
    download_and_verify "${base_url}"
    extract_package
    ensure_service_user
    install_files
    start_service

    log "安装完成：$(${BINARY_PATH} --version 2>/dev/null || printf 'pulse')"
    log "Web 控制台：http://127.0.0.1:8080"
}

main "$@"
