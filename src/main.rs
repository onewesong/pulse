use std::{path::PathBuf, sync::Arc};

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use pulse::{
    collector::{run_cleanup, run_event_listener, run_sampler},
    config::{AppConfig, ensure_database_parent},
    db::Database,
    web::{AppState, router},
};
use tokio_util::sync::CancellationToken;
use tracing::{error, info};
use tracing_subscriber::EnvFilter;

#[derive(Debug, Parser)]
#[command(name = "pulse", version, about = "Docker 容器指标采集与趋势查看工具")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// 启动指标采集器和 Web 控制台
    Serve {
        /// TOML 配置文件路径；省略时尝试 /etc/pulse/config.toml
        #[arg(long)]
        config: Option<PathBuf>,
    },
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| EnvFilter::new("pulse=info,tower_http=info")),
        )
        .with_target(false)
        .compact()
        .init();

    let cli = Cli::parse();
    match cli.command {
        Command::Serve { config } => serve(config).await,
    }
}

async fn serve(config_path: Option<PathBuf>) -> Result<()> {
    let config = Arc::new(AppConfig::load(config_path.as_deref()).context("加载配置失败")?);
    ensure_database_parent(&config.database.path)?;
    let db = Database::connect(&config.database.path).await?;
    let listener = tokio::net::TcpListener::bind(&config.web.listen)
        .await
        .with_context(|| format!("无法监听 {}", config.web.listen))?;
    let cancel = CancellationToken::new();

    let sampler = tokio::spawn(run_sampler(
        db.clone(),
        config.clone(),
        cancel.child_token(),
    ));
    let event_listener = tokio::spawn(run_event_listener(
        db.clone(),
        config.clone(),
        cancel.child_token(),
    ));
    let cleanup = tokio::spawn(run_cleanup(
        db.clone(),
        config.collector.retention_days,
        cancel.child_token(),
    ));

    let app = router(AppState {
        db,
        retention_days: config.collector.retention_days,
        max_points: config.web.max_points,
        collector_interval_seconds: config.collector.interval_seconds,
    });

    info!(listen = %config.web.listen, database = %config.database.path.display(), "Pulse 已启动");
    let shutdown = cancel.clone();
    tokio::spawn(async move {
        shutdown_signal().await;
        info!("收到退出信号，正在优雅关闭");
        shutdown.cancel();
    });

    let server_result = axum::serve(listener, app)
        .with_graceful_shutdown(cancel.clone().cancelled_owned())
        .await;
    cancel.cancel();
    for task in [sampler, event_listener, cleanup] {
        if let Err(error) = task.await {
            error!(%error, "后台任务异常退出");
        }
    }
    server_result.context("Web 服务异常退出")
}

async fn shutdown_signal() {
    let ctrl_c = async {
        tokio::signal::ctrl_c()
            .await
            .expect("无法安装 Ctrl+C 信号处理器");
    };

    #[cfg(unix)]
    let terminate = async {
        tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            .expect("无法安装 SIGTERM 信号处理器")
            .recv()
            .await;
    };

    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! { _ = ctrl_c => {}, _ = terminate => {} }
}
