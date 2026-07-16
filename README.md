# Pulse

Pulse 是一个轻量的单机 Docker 指标采集器。它定时把容器 CPU、内存、网络、块 I/O、PID 和生命周期事件写入 SQLite，并通过一个无需外部资源的 Web 控制台展示趋势。

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
make release    # 构建 release 二进制
make run        # 使用 .data/pulse.db 启动本地服务
```

也可以直接使用 Cargo：

```bash
cargo build --release
mkdir -p ./data
PULSE_DATABASE__PATH=./data/pulse.db \
  PULSE_WEB__LISTEN=127.0.0.1:8080 \
  ./target/release/pulse serve
```

打开 `http://127.0.0.1:8080`。默认只监听本机；远程查看建议使用 SSH 隧道或带认证的反向代理。

也可以显式指定配置文件：

```bash
./target/release/pulse serve --config ./config/pulse.toml
```

## 配置

完整示例见 [`config/pulse.toml`](config/pulse.toml)。配置优先级为环境变量、TOML、内置默认值。环境变量以 `PULSE_` 开头，层级使用双下划线，例如：

```bash
PULSE_COLLECTOR__INTERVAL_SECONDS=30
PULSE_FILTERS__EXCLUDE_LABELS='pulse.ignore=true,team=temporary'
```

label 选择器支持 `key` 和 `key=value`。`include_labels` 为空时包含所有容器，否则任意一个选择器匹配即可；任意 `exclude_labels` 匹配都会排除该容器。

## systemd 安装

```bash
cargo build --release
sudo useradd --system --home /var/lib/pulse --shell /usr/sbin/nologin pulse
sudo install -Dm755 target/release/pulse /usr/local/bin/pulse
sudo install -Dm640 config/pulse.toml /etc/pulse/config.toml
sudo install -Dm644 packaging/pulse.service /etc/systemd/system/pulse.service
sudo systemctl daemon-reload
sudo systemctl enable --now pulse
```

`pulse` 用户通过 `SupplementaryGroups=docker` 访问 `/var/run/docker.sock`。Docker Socket 可以控制宿主机上的容器，通常等同于接近 root 的权限；请只在可信主机上运行，并保护 Pulse 服务账号。

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
