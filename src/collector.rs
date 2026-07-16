use std::{collections::HashMap, sync::Arc, time::Duration};

use anyhow::{Context, Result, anyhow};
use bollard::{
    API_DEFAULT_VERSION, Docker,
    models::{ContainerStatsResponse, ContainerSummary, EventMessageTypeEnum},
    query_parameters::{EventsOptionsBuilder, ListContainersOptionsBuilder, StatsOptionsBuilder},
};
use futures_util::{StreamExt, TryStreamExt, stream};
use tokio::{
    sync::Semaphore,
    time::{MissedTickBehavior, timeout},
};
use tokio_util::sync::CancellationToken;
use tracing::{debug, error, info, warn};

use crate::{
    config::{AppConfig, FilterConfig, labels_match},
    db::Database,
    models::{ContainerMetadata, MetricSample, NewContainerEvent, Observation},
};

pub async fn run_sampler(db: Database, config: Arc<AppConfig>, cancel: CancellationToken) {
    let mut ticker = tokio::time::interval(Duration::from_secs(config.collector.interval_seconds));
    ticker.set_missed_tick_behavior(MissedTickBehavior::Skip);
    loop {
        tokio::select! {
            biased;
            _ = cancel.cancelled() => break,
            _ = ticker.tick() => {
                if let Err(error) = collect_once(&db, &config).await {
                    error!(%error, "Docker 指标采集失败");
                    let _ = db.set_collector_status(false, Some(&format!("{error:#}"))).await;
                }
            }
        }
    }
    debug!("指标采集任务已停止");
}

pub async fn run_event_listener(db: Database, config: Arc<AppConfig>, cancel: CancellationToken) {
    let mut backoff = Duration::from_secs(1);
    let mut since = chrono::Utc::now().timestamp().to_string();

    while !cancel.is_cancelled() {
        match docker_client(&config) {
            Ok(docker) => {
                let event_filters = HashMap::from([("type", vec!["container"])]);
                let options = EventsOptionsBuilder::default()
                    .since(&since)
                    .filters(&event_filters)
                    .build();
                let mut events = Box::pin(docker.events(Some(options)));
                backoff = Duration::from_secs(1);
                loop {
                    tokio::select! {
                        _ = cancel.cancelled() => return,
                        event = events.try_next() => match event {
                            Ok(Some(event)) => {
                                if let Some(time) = event.time {
                                    // 重连时允许重复拉取同一秒，数据库唯一键会去重，避免漏掉同秒事件。
                                    since = time.to_string();
                                }
                                if let Some(event) = convert_event(event, &config.filters)
                                    && let Err(error) = db.write_event(&event).await
                                {
                                    warn!(%error, "保存 Docker 事件失败");
                                }
                            }
                            Ok(None) => break,
                            Err(error) => {
                                warn!(%error, "Docker 事件流断开");
                                break;
                            }
                        }
                    }
                }
            }
            Err(error) => warn!(%error, "无法连接 Docker 事件流"),
        }

        tokio::select! {
            _ = cancel.cancelled() => break,
            _ = tokio::time::sleep(backoff) => {}
        }
        backoff = (backoff * 2).min(Duration::from_secs(60));
    }
}

pub async fn run_cleanup(db: Database, retention_days: u64, cancel: CancellationToken) {
    let mut ticker = tokio::time::interval(Duration::from_secs(60 * 60));
    ticker.set_missed_tick_behavior(MissedTickBehavior::Skip);
    loop {
        tokio::select! {
            _ = cancel.cancelled() => break,
            _ = ticker.tick() => {
                let age_ms = (retention_days as i64).saturating_mul(86_400_000);
                let cutoff = chrono::Utc::now().timestamp_millis().saturating_sub(age_ms);
                match db.cleanup_before(cutoff).await {
                    Ok((metrics, events, containers)) => {
                        info!(metrics, events, containers, "历史数据清理完成");
                    }
                    Err(error) => warn!(%error, "历史数据清理失败"),
                }
            }
        }
    }
}

async fn collect_once(db: &Database, config: &AppConfig) -> Result<()> {
    let docker = docker_client(config)?;
    docker.ping().await.context("Docker ping 失败")?;
    let containers = docker
        .list_containers(Some(
            ListContainersOptionsBuilder::default().all(true).build(),
        ))
        .await
        .context("列出 Docker 容器失败")?;
    let now = chrono::Utc::now().timestamp_millis();
    let selected: Vec<_> = containers
        .into_iter()
        .filter(|container| {
            labels_match(
                container.labels.as_ref().unwrap_or(&HashMap::new()),
                &config.filters,
            )
        })
        .collect();

    let semaphore = Arc::new(Semaphore::new(config.collector.concurrency));
    let observations = stream::iter(selected.into_iter().map(|container| {
        let docker = docker.clone();
        let semaphore = semaphore.clone();
        let timeout_duration = Duration::from_secs(config.docker.timeout_seconds);
        async move {
            let metadata = metadata_from_summary(&container, now)?;
            if metadata.state != "running" {
                return Ok::<_, anyhow::Error>(Observation {
                    container: metadata,
                    sample: None,
                });
            }
            let _permit = semaphore.acquire_owned().await?;
            let id = metadata.docker_id.clone();
            let stat = timeout(timeout_duration, read_stats(&docker, &id)).await;
            let sample = match stat {
                Ok(Ok(stats)) => Some(sample_from_stats(&id, now, &stats)),
                Ok(Err(error)) => {
                    warn!(container_id = %id, %error, "读取容器指标失败");
                    None
                }
                Err(_) => {
                    warn!(container_id = %id, "读取容器指标超时");
                    None
                }
            };
            Ok(Observation {
                container: metadata,
                sample,
            })
        }
    }))
    .buffer_unordered(config.collector.concurrency)
    .filter_map(|result| async {
        match result {
            Ok(observation) => Some(observation),
            Err(error) => {
                warn!(%error, "忽略无效的容器记录");
                None
            }
        }
    })
    .collect::<Vec<_>>()
    .await;

    db.write_observations(&observations)
        .await
        .context("批量保存采集结果失败")?;
    db.set_collector_status(true, None).await?;
    debug!(containers = observations.len(), "Docker 指标采集完成");
    Ok(())
}

fn docker_client(config: &AppConfig) -> Result<Docker> {
    Docker::connect_with_unix(
        &config.docker.socket,
        config.docker.timeout_seconds,
        API_DEFAULT_VERSION,
    )
    .context("创建 Docker Socket 客户端失败")
}

async fn read_stats(docker: &Docker, id: &str) -> Result<ContainerStatsResponse> {
    let mut stream = docker.stats(
        id,
        Some(
            StatsOptionsBuilder::default()
                .stream(false)
                .one_shot(false)
                .build(),
        ),
    );
    stream
        .try_next()
        .await?
        .ok_or_else(|| anyhow!("Docker 未返回指标"))
}

fn metadata_from_summary(summary: &ContainerSummary, seen_at_ms: i64) -> Result<ContainerMetadata> {
    let docker_id = summary
        .id
        .clone()
        .ok_or_else(|| anyhow!("容器记录缺少 ID"))?;
    let name = summary
        .names
        .as_ref()
        .and_then(|names| names.first())
        .map(|name| name.trim_start_matches('/').to_string())
        .unwrap_or_else(|| docker_id.chars().take(12).collect());
    Ok(ContainerMetadata {
        docker_id,
        name,
        image: summary.image.clone().unwrap_or_default(),
        labels: summary.labels.clone().unwrap_or_default(),
        state: summary
            .state
            .map(|state| state.to_string())
            .unwrap_or_else(|| "unknown".into()),
        status: summary.status.clone().unwrap_or_default(),
        docker_created_at_ms: summary.created.map(|seconds| seconds.saturating_mul(1000)),
        seen_at_ms,
    })
}

pub(crate) fn sample_from_stats(
    id: &str,
    sampled_at_ms: i64,
    stats: &ContainerStatsResponse,
) -> MetricSample {
    let cpu_percent = cpu_percent(stats);
    let (memory_working_set_bytes, memory_limit_bytes, memory_percent) = memory_values(stats);
    let (network_rx_bytes, network_tx_bytes) = network_values(stats);
    let (block_read_bytes, block_write_bytes) = block_values(stats);
    let pids = stats
        .pids_stats
        .as_ref()
        .and_then(|pids| pids.current)
        .map(saturating_i64);

    MetricSample {
        container_id: id.to_string(),
        sampled_at_ms,
        cpu_percent,
        memory_working_set_bytes,
        memory_limit_bytes,
        memory_percent,
        network_rx_bytes,
        network_tx_bytes,
        block_read_bytes,
        block_write_bytes,
        pids,
    }
}

fn cpu_percent(stats: &ContainerStatsResponse) -> Option<f64> {
    let current = stats.cpu_stats.as_ref()?;
    let previous = stats.precpu_stats.as_ref()?;
    let cpu_delta = current
        .cpu_usage
        .as_ref()?
        .total_usage?
        .checked_sub(previous.cpu_usage.as_ref()?.total_usage?)?;
    let system_delta = current
        .system_cpu_usage?
        .checked_sub(previous.system_cpu_usage?)?;
    if system_delta == 0 {
        return None;
    }
    let online_cpus = current
        .online_cpus
        .map(u64::from)
        .or_else(|| {
            current
                .cpu_usage
                .as_ref()?
                .percpu_usage
                .as_ref()
                .map(|cpus| cpus.len() as u64)
        })
        .unwrap_or(1);
    Some(cpu_delta as f64 / system_delta as f64 * online_cpus as f64 * 100.0)
}

fn memory_values(stats: &ContainerStatsResponse) -> (Option<i64>, Option<i64>, Option<f64>) {
    let Some(memory) = stats.memory_stats.as_ref() else {
        return (None, None, None);
    };
    let usage = memory.usage;
    let cache = memory
        .stats
        .as_ref()
        .and_then(|values| {
            values
                .get("total_inactive_file")
                .or_else(|| values.get("inactive_file"))
                .copied()
        })
        .unwrap_or(0);
    let working_set = usage.map(|value| value.saturating_sub(cache));
    let limit = memory.limit.filter(|limit| *limit > 0);
    let percent = working_set
        .zip(limit)
        .map(|(used, limit)| used as f64 / limit as f64 * 100.0);
    (
        working_set.map(saturating_i64),
        limit.map(saturating_i64),
        percent,
    )
}

fn network_values(stats: &ContainerStatsResponse) -> (Option<i64>, Option<i64>) {
    let Some(networks) = stats.networks.as_ref() else {
        return (None, None);
    };
    let rx = networks
        .values()
        .filter_map(|network| network.rx_bytes)
        .fold(0_u64, u64::saturating_add);
    let tx = networks
        .values()
        .filter_map(|network| network.tx_bytes)
        .fold(0_u64, u64::saturating_add);
    (Some(saturating_i64(rx)), Some(saturating_i64(tx)))
}

fn block_values(stats: &ContainerStatsResponse) -> (Option<i64>, Option<i64>) {
    let Some(entries) = stats
        .blkio_stats
        .as_ref()
        .and_then(|stats| stats.io_service_bytes_recursive.as_ref())
    else {
        return (None, None);
    };
    let mut read = 0_u64;
    let mut write = 0_u64;
    for entry in entries {
        match entry.op.as_deref().map(str::to_ascii_lowercase).as_deref() {
            Some("read") => read = read.saturating_add(entry.value.unwrap_or(0)),
            Some("write") => write = write.saturating_add(entry.value.unwrap_or(0)),
            _ => {}
        }
    }
    (Some(saturating_i64(read)), Some(saturating_i64(write)))
}

fn convert_event(
    event: bollard::models::EventMessage,
    filters: &FilterConfig,
) -> Option<NewContainerEvent> {
    if event.typ != Some(EventMessageTypeEnum::CONTAINER) {
        return None;
    }
    let actor = event.actor?;
    let id = actor.id?;
    let attributes = actor.attributes.unwrap_or_default();
    if !labels_match(&attributes, filters) {
        return None;
    }
    let action = event.action.unwrap_or_else(|| "unknown".into());
    if !matches!(
        action.as_str(),
        "create"
            | "start"
            | "stop"
            | "die"
            | "kill"
            | "destroy"
            | "restart"
            | "pause"
            | "unpause"
            | "oom"
    ) {
        return None;
    }
    let state = match action.as_str() {
        "start" | "restart" | "unpause" => "running",
        "pause" => "paused",
        "create" => "created",
        "stop" | "die" | "kill" | "destroy" => "exited",
        "oom" => "running",
        _ => unreachable!(),
    };
    let occurred_at_ms = event
        .time_nano
        .map(|nanos| nanos / 1_000_000)
        .or_else(|| event.time.map(|seconds| seconds.saturating_mul(1000)))
        .unwrap_or_else(|| chrono::Utc::now().timestamp_millis());
    let exit_code = attributes
        .get("exitCode")
        .and_then(|value| value.parse().ok());
    Some(NewContainerEvent {
        container: ContainerMetadata {
            docker_id: id,
            name: attributes
                .get("name")
                .cloned()
                .unwrap_or_else(|| "unknown".into()),
            image: attributes.get("image").cloned().unwrap_or_default(),
            labels: attributes.clone(),
            state: state.into(),
            status: action.clone(),
            docker_created_at_ms: None,
            seen_at_ms: occurred_at_ms,
        },
        event_type: "container".into(),
        action: action.clone(),
        occurred_at_ms,
        exit_code,
        oom_killed: action == "oom",
        attributes,
    })
}

fn saturating_i64(value: u64) -> i64 {
    value.min(i64::MAX as u64) as i64
}

#[cfg(test)]
mod tests {
    use super::*;
    use bollard::models::{
        ContainerCpuStats, ContainerCpuUsage, ContainerMemoryStats, ContainerNetworkStats,
    };

    #[test]
    fn calculates_cpu_memory_and_network() {
        let stats = ContainerStatsResponse {
            cpu_stats: Some(ContainerCpuStats {
                cpu_usage: Some(ContainerCpuUsage {
                    total_usage: Some(300),
                    ..Default::default()
                }),
                system_cpu_usage: Some(2_000),
                online_cpus: Some(2),
                ..Default::default()
            }),
            precpu_stats: Some(ContainerCpuStats {
                cpu_usage: Some(ContainerCpuUsage {
                    total_usage: Some(100),
                    ..Default::default()
                }),
                system_cpu_usage: Some(1_000),
                ..Default::default()
            }),
            memory_stats: Some(ContainerMemoryStats {
                usage: Some(1_000),
                limit: Some(2_000),
                stats: Some(HashMap::from([("inactive_file".into(), 200)])),
                ..Default::default()
            }),
            networks: Some(HashMap::from([(
                "eth0".into(),
                ContainerNetworkStats {
                    rx_bytes: Some(10),
                    tx_bytes: Some(20),
                    ..Default::default()
                },
            )])),
            ..Default::default()
        };
        let sample = sample_from_stats("id", 1, &stats);
        assert_eq!(sample.cpu_percent, Some(40.0));
        assert_eq!(sample.memory_working_set_bytes, Some(800));
        assert_eq!(sample.memory_percent, Some(40.0));
        assert_eq!(sample.network_rx_bytes, Some(10));
    }

    #[test]
    fn invalid_deltas_produce_no_cpu_spike() {
        let stats = ContainerStatsResponse {
            cpu_stats: Some(ContainerCpuStats {
                cpu_usage: Some(ContainerCpuUsage {
                    total_usage: Some(1),
                    ..Default::default()
                }),
                system_cpu_usage: Some(1),
                ..Default::default()
            }),
            precpu_stats: Some(ContainerCpuStats {
                cpu_usage: Some(ContainerCpuUsage {
                    total_usage: Some(2),
                    ..Default::default()
                }),
                system_cpu_usage: Some(2),
                ..Default::default()
            }),
            ..Default::default()
        };
        assert_eq!(sample_from_stats("id", 1, &stats).cpu_percent, None);
    }
}
