SHELL := /bin/bash

CARGO ?= cargo
PREFIX ?= /usr/local
SYSCONFDIR ?= /etc
SYSTEMD_UNIT_DIR ?= /etc/systemd/system
DESTDIR ?=
RUST_LOG ?= pulse=info,tower_http=info

BINARY := pulse
RELEASE_BINARY := target/release/$(BINARY)
CONFIG_SOURCE := config/pulse.toml
SERVICE_SOURCE := packaging/pulse.service

.DEFAULT_GOAL := help

.PHONY: help build release fmt fmt-check test clippy check run clean install uninstall

help: ## 显示可用命令
	@awk 'BEGIN {FS = ":.*## "; printf "Pulse 常用命令：\n\n"} /^[a-zA-Z_-]+:.*## / {printf "  %-14s %s\n", $$1, $$2}' $(MAKEFILE_LIST)

build: ## 构建开发版本
	$(CARGO) build

release: ## 构建优化后的 release 二进制
	$(CARGO) build --release

fmt: ## 格式化 Rust 源码
	$(CARGO) fmt --all

fmt-check: ## 检查 Rust 源码格式
	$(CARGO) fmt --all --check

test: ## 运行全部测试
	$(CARGO) test --all-targets

clippy: ## 运行严格 Clippy 检查
	$(CARGO) clippy --all-targets -- -D warnings

check: fmt-check test clippy ## 运行提交前的全部质量检查

run: ## 使用 .data/pulse.db 启动本地开发服务
	@mkdir -p .data
	PULSE_DATABASE__PATH=$(CURDIR)/.data/pulse.db \
	PULSE_WEB__LISTEN=127.0.0.1:8080 \
	RUST_LOG=$(RUST_LOG) \
	$(CARGO) run -- serve

clean: ## 清理 Cargo 构建产物
	$(CARGO) clean

install: release ## 安装二进制、示例配置和 systemd 单元
	install -Dm755 $(RELEASE_BINARY) $(DESTDIR)$(PREFIX)/bin/$(BINARY)
	install -Dm640 $(CONFIG_SOURCE) $(DESTDIR)$(SYSCONFDIR)/pulse/config.toml
	install -Dm644 $(SERVICE_SOURCE) $(DESTDIR)$(SYSTEMD_UNIT_DIR)/pulse.service

uninstall: ## 删除由 make install 安装的文件
	rm -f $(DESTDIR)$(PREFIX)/bin/$(BINARY)
	rm -f $(DESTDIR)$(SYSCONFDIR)/pulse/config.toml
	rm -f $(DESTDIR)$(SYSTEMD_UNIT_DIR)/pulse.service

