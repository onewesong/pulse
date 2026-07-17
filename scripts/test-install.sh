#!/usr/bin/env bash
set -Eeuo pipefail

PROJECT_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
readonly PROJECT_ROOT
TEST_ROOT="$(mktemp -d)"

cleanup() {
    rm -rf "${TEST_ROOT}"
}
trap cleanup EXIT

fail() {
    printf '安装脚本测试失败：%s\n' "$*" >&2
    exit 1
}

assert_file() {
    [[ -f "$1" ]] || fail "缺少文件 $1"
}

assert_contains() {
    grep -Fq "$2" "$1" || fail "$1 不包含：$2"
}

create_release_fixture() {
    local release_dir="$1"
    local package="pulse-v0.1.2-x86_64-unknown-linux-musl"
    mkdir -p "${release_dir}/${package}/config" "${release_dir}/${package}/packaging"
    cp "${PROJECT_ROOT}/config/pulse.toml" "${release_dir}/${package}/config/"
    cp "${PROJECT_ROOT}/packaging/pulse.service" "${release_dir}/${package}/packaging/"
    cp "${PROJECT_ROOT}/LICENSE" "${release_dir}/${package}/"
    printf '#!/usr/bin/env bash\nprintf "pulse 0.1.2\\n"\n' >"${release_dir}/${package}/pulse"
    chmod 755 "${release_dir}/${package}/pulse"
    tar -C "${release_dir}" -czf "${release_dir}/pulse-x86_64-unknown-linux-musl.tar.gz" "${package}"
    rm -rf "${release_dir:?}/${package}"
    (cd "${release_dir}" && sha256sum pulse-x86_64-unknown-linux-musl.tar.gz >SHA256SUMS)
}

create_mock_commands() {
    local mock_bin="$1"
    mkdir -p "${mock_bin}"

    cat >"${mock_bin}/id" <<'EOF'
#!/usr/bin/env bash
if [[ "$#" == 1 && "$1" == "-u" ]]; then
    echo 0
elif [[ "$#" == 2 && "$1" == "-u" && "$2" == "pulse" ]]; then
    [[ -f "${MOCK_STATE}/user-created" ]] || exit 1
    echo 991
else
    /usr/bin/id "$@"
fi
EOF
    cat >"${mock_bin}/useradd" <<'EOF'
#!/usr/bin/env bash
touch "${MOCK_STATE}/user-created"
printf 'useradd %s\n' "$*" >>"${MOCK_STATE}/commands.log"
EOF
    cat >"${mock_bin}/usermod" <<'EOF'
#!/usr/bin/env bash
printf 'usermod %s\n' "$*" >>"${MOCK_STATE}/commands.log"
EOF
    cat >"${mock_bin}/getent" <<'EOF'
#!/usr/bin/env bash
[[ "$1" == "group" && "$2" == "docker" ]] && printf 'docker:x:999:\n'
EOF
    cat >"${mock_bin}/docker" <<'EOF'
#!/usr/bin/env bash
exit 0
EOF
    cat >"${mock_bin}/systemctl" <<'EOF'
#!/usr/bin/env bash
printf 'systemctl %s\n' "$*" >>"${MOCK_STATE}/commands.log"
exit 0
EOF
    cat >"${mock_bin}/journalctl" <<'EOF'
#!/usr/bin/env bash
printf 'journalctl %s\n' "$*" >>"${MOCK_STATE}/commands.log"
EOF
    cat >"${mock_bin}/chown" <<'EOF'
#!/usr/bin/env bash
printf 'chown %s\n' "$*" >>"${MOCK_STATE}/commands.log"
EOF
    cat >"${mock_bin}/uname" <<'EOF'
#!/usr/bin/env bash
if [[ "$1" == "-m" && -n "${MOCK_UNAME_M:-}" ]]; then
    printf '%s\n' "${MOCK_UNAME_M}"
else
    /usr/bin/uname "$@"
fi
EOF
    chmod 755 "${mock_bin}"/*
}

run_installer() {
    env \
        PATH="${MOCK_BIN}:${PATH}" \
        MOCK_STATE="${MOCK_STATE}" \
        PULSE_INSTALL_ROOT="${INSTALL_ROOT}" \
        PULSE_DOCKER_SOCKET="${DOCKER_SOCKET}" \
        PULSE_RELEASE_BASE_URL="file://${RELEASE_DIR}" \
        "$@" \
        bash "${PROJECT_ROOT}/scripts/install.sh"
}

RELEASE_DIR="${TEST_ROOT}/release"
MOCK_BIN="${TEST_ROOT}/mock-bin"
MOCK_STATE="${TEST_ROOT}/mock-state"
INSTALL_ROOT="${TEST_ROOT}/root"
DOCKER_SOCKET="${TEST_ROOT}/docker.sock"
mkdir -p "${RELEASE_DIR}" "${MOCK_STATE}"
create_release_fixture "${RELEASE_DIR}"
create_mock_commands "${MOCK_BIN}"

# Unix Socket 文件必须真实存在，安装器才会继续。
DOCKER_PID=""
python3 - "${DOCKER_SOCKET}" <<'PY' &
import socket
import sys
import time

server = socket.socket(socket.AF_UNIX)
server.bind(sys.argv[1])
server.listen(1)
time.sleep(60)
PY
DOCKER_PID=$!
trap '[[ -n "${DOCKER_PID}" ]] && kill "${DOCKER_PID}" 2>/dev/null || true; cleanup' EXIT
for _ in {1..50}; do
    [[ -S "${DOCKER_SOCKET}" ]] && break
    sleep 0.02
done
[[ -S "${DOCKER_SOCKET}" ]] || fail "无法创建测试 Docker Socket"

run_installer >"${TEST_ROOT}/first-install.log"
assert_file "${INSTALL_ROOT}/usr/local/bin/pulse"
assert_file "${INSTALL_ROOT}/etc/pulse/config.toml"
assert_file "${INSTALL_ROOT}/etc/systemd/system/pulse.service"
[[ "$(stat -c '%a' "${INSTALL_ROOT}/etc/pulse/config.toml")" == "640" ]] || fail "默认配置权限不是 0640"
assert_contains "${MOCK_STATE}/commands.log" "systemctl restart pulse.service"
assert_contains "${MOCK_STATE}/commands.log" "chown root:pulse ${INSTALL_ROOT}/etc/pulse/config.toml"
assert_contains "${TEST_ROOT}/first-install.log" "SHA-256 校验通过"

printf '# 用户自定义配置\n' >"${INSTALL_ROOT}/etc/pulse/config.toml"
run_installer >"${TEST_ROOT}/upgrade.log"
assert_contains "${INSTALL_ROOT}/etc/pulse/config.toml" "用户自定义配置"
assert_contains "${TEST_ROOT}/upgrade.log" "保留现有配置"
[[ "$(grep -c '^useradd ' "${MOCK_STATE}/commands.log")" == 1 ]] || fail "升级时重复创建了用户"

cp "${RELEASE_DIR}/pulse-x86_64-unknown-linux-musl.tar.gz" "${RELEASE_DIR}/tampered.tar.gz"
printf 'tampered' >>"${RELEASE_DIR}/pulse-x86_64-unknown-linux-musl.tar.gz"
if run_installer >"${TEST_ROOT}/checksum.log" 2>&1; then
    fail "损坏的发布包未被拒绝"
fi
assert_contains "${TEST_ROOT}/checksum.log" "SHA-256 校验失败"
mv "${RELEASE_DIR}/tampered.tar.gz" "${RELEASE_DIR}/pulse-x86_64-unknown-linux-musl.tar.gz"

if run_installer MOCK_UNAME_M=aarch64 >"${TEST_ROOT}/architecture.log" 2>&1; then
    fail "不支持的架构未被拒绝"
fi
assert_contains "${TEST_ROOT}/architecture.log" "当前仅支持 x86_64"

printf '安装脚本测试通过\n'
