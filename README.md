<div align="center">

# Pulse

**A lightweight Docker metrics collector and dashboard for a single host.**

[![CI](https://github.com/onewesong/pulse/actions/workflows/ci.yml/badge.svg)](https://github.com/onewesong/pulse/actions/workflows/ci.yml)
[![GitHub Release](https://img.shields.io/github/v/release/onewesong/pulse?style=flat-square&color=orange)](https://github.com/onewesong/pulse/releases/latest)
[![Docker Image](https://img.shields.io/badge/ghcr.io-onewesong%2Fpulse-blue?style=flat-square&logo=docker&logoColor=white)](https://github.com/onewesong/pulse/pkgs/container/pulse)
[![Platform](https://img.shields.io/badge/platform-Linux%20x86__64-2496ED?style=flat-square&logo=linux&logoColor=white)](https://github.com/onewesong/pulse/releases/latest)
[![Rust](https://img.shields.io/badge/Rust-2024-000000?style=flat-square&logo=rust&logoColor=white)](https://www.rust-lang.org/)
[![License](https://img.shields.io/github/license/onewesong/pulse?style=flat-square&color=brightgreen)](LICENSE)

</div>

---

Pulse 是一个轻量的单机 Docker 指标采集器。它定时把容器 CPU、内存、网络、块 I/O、PID 和生命周期事件写入 SQLite，并通过无需外部资源的 Web 控制台展示趋势。

首版不调用任何 AI 模型；`/api/v1/analysis/context` 会输出带统计摘要、事件和降采样序列的稳定 JSON，后续可以直接交给模型分析。

## 功能

- 默认每 15 秒采集本机所有 Docker 容器，保存 30 天。
- 容器列表、当前资源排行以及 1 小时至 30 天趋势。
- CPU、内存工作集、网络收发速率、块 I/O 速率和 PID。
- 启动、停止、重启、OOM、退出等 Docker 事件。
- SQLite WAL、批量事务、自动迁移和每小时历史清理。
- Docker 离线时继续查看历史数据，并显示采集器降级状态。
- 内嵌 HTML、CSS 和 Canvas 图表，不依赖 Node.js、CDN 或浏览器联网。

## 快速开始

要求 Linux、Docker Engine 和稳定版 Rust。当前用户必须能访问 Docker Socket。

推荐使用 Makefile：

```bash
make check      # 格式、测试和 Clippy
make release-static # 构建可兼容旧系统的静态 release 二进制
make run        # 使用 .data/pulse.db 启动本地服务
```

也可以直接使用 Cargo：

```bash
rustup target add x86_64-unknown-linux-musl
# Ubuntu / Debian: sudo apt-get install musl-tools
CC_x86_64_unknown_linux_musl=musl-gcc \
  cargo build --release --locked --target x86_64-unknown-linux-musl
mkdir -p ./data
PULSE_DATABASE__PATH=./data/pulse.db \
  PULSE_WEB__LISTEN=127.0.0.1:8080 \
  ./target/x86_64-unknown-linux-musl/release/pulse serve
```

打开 `http://127.0.0.1:8080`。默认只监听本机；远程查看建议使用 SSH 隧道或带认证的反向代理。

也可以显式指定配置文件：

```bash
./target/x86_64-unknown-linux-musl/release/pulse serve --config ./config/pulse.toml
```

## 配置

完整示例见 [`config/pulse.toml`](config/pulse.toml)。配置优先级为环境变量、TOML、内置默认值。环境变量以 `PULSE_` 开头，层级使用双下划线，例如：

```bash
PULSE_COLLECTOR__INTERVAL_SECONDS=30
PULSE_FILTERS__EXCLUDE_LABELS='pulse.ignore=true,team=temporary'
```

label 选择器支持 `key` 和 `key=value`。`include_labels` 为空时包含所有容器，否则任意一个选择器匹配即可；任意 `exclude_labels` 匹配都会排除该容器。

## Docker

镜像托管于 GitHub Container Registry，同时支持 `linux/amd64` 和 `linux/arm64`：

```bash
docker run -d \
  --name pulse \
  --restart unless-stopped \
  --label pulse.ignore=true \
  -p 127.0.0.1:8080:8080 \
  -v pulse-data:/data \
  -v /var/run/docker.sock:/var/run/docker.sock:ro \
  ghcr.io/onewesong/pulse:latest
```

也可以直接使用仓库中的 Compose 配置：

```bash
docker compose up -d
docker compose logs -f pulse
```

指标数据库保存在 `pulse-data` 数据卷中。容器默认监听 `0.0.0.0:8080`，示例仅将其映射到宿主机 `127.0.0.1:8080`。

> [!WARNING]
> 挂载 Docker Socket 即使标记为只读，也仍可能通过 Docker API 控制宿主机，权限通常接近 root。请只运行可信镜像，不要将 Pulse Web 端口直接暴露到公网。

镜像标签规则：

- 推送到 `main`：发布 `edge` 和 `sha-*`。
- 推送 `v1.2.3` 标签：发布 `v1.2.3`、`1.2.3`、`1.2`、`1` 和 `latest`。
- 预发布标签不会覆盖 `latest`。

## 一键安装

一键安装要求 Linux x86_64、systemd 和已运行的 Docker Engine。脚本会下载最新稳定 Release、验证 SHA-256、创建 `pulse` 系统用户，并启用服务：

```bash
curl -fsSL https://github.com/onewesong/pulse/releases/latest/download/install.sh | sudo bash
```

指定版本：

```bash
curl -fsSL https://github.com/onewesong/pulse/releases/latest/download/install.sh \
  | sudo env VERSION=0.1.2 bash
```

重复运行脚本会升级二进制并重启服务，但保留现有 `/etc/pulse/config.toml`。安装完成后打开 `http://127.0.0.1:8080`。

## systemd 安装

```bash
make release-static
sudo useradd --system --home /var/lib/pulse --shell /usr/sbin/nologin pulse
sudo install -Dm755 target/x86_64-unknown-linux-musl/release/pulse /usr/local/bin/pulse
sudo install -Dm640 config/pulse.toml /etc/pulse/config.toml
sudo chown root:pulse /etc/pulse/config.toml
sudo install -Dm644 packaging/pulse.service /etc/systemd/system/pulse.service
sudo systemctl daemon-reload
sudo systemctl enable --now pulse
```

`pulse` 用户通过 `SupplementaryGroups=docker` 访问 `/var/run/docker.sock`。Docker Socket 可以控制宿主机上的容器，通常等同于接近 root 的权限；请只在可信主机上运行，并保护 Pulse 服务账号。

## 发布版本

推送与 `Cargo.toml` 版本一致的 `v*` 标签后，GitHub Actions 会自动执行格式检查、测试和 Clippy，构建不依赖宿主机 glibc 的 Linux x86_64 musl 静态包、发布多架构 GHCR 镜像，并创建带 SHA-256 校验文件的 GitHub Release。

```bash
# 先更新 Cargo.toml 和 Cargo.lock 中的版本并提交
make tag VERSION=0.1.2
git push origin v0.1.2
```

标签包含预发布后缀时（例如 `v0.2.0-rc.1`），生成的 GitHub Release 会自动标记为 Pre-release。标签版本与 `Cargo.toml` 不一致时，工作流会拒绝发布。

## HTTP API

- `GET /api/v1/containers`
- `GET /api/v1/containers/{id}/metrics?from=<毫秒>&to=<毫秒>&max_points=2000`
- `GET /api/v1/containers/{id}/events?from=<毫秒>&to=<毫秒>`
- `GET /api/v1/analysis/context?container_ids=<id1,id2>&from=<毫秒>&to=<毫秒>`
- `GET /api/v1/system/status`
- `GET /health/live` 和 `GET /health/ready`

容器 ID 可以使用无歧义的十六进制前缀。查询范围不能超过配置的保留期，分析接口一次最多接受 10 个容器。

## 开发与测试

```bash
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test --all-targets
```

本机烟雾测试可以使用开发数据库运行服务，然后访问健康检查和 API。测试结束后发送 `SIGTERM`，服务会停止新采集、完成当前事务并优雅退出。

## 数据语义

- CPU 使用率按 Docker 当前/前次容器 CPU 与系统 CPU 时间差计算，可以超过 100%（表示使用多个核心）。
- 内存采用 `usage - inactive_file` 工作集口径。
- 网络和块 I/O 在库中保存累计值，API 按时间点差值转换为每秒速率；首次采样或计数器回退返回 `null`。
- 同名容器重建后具有不同 Docker ID，因此历史不会混合。
