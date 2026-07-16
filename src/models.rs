use std::collections::HashMap;

use serde::Serialize;
use sqlx::FromRow;

#[derive(Debug, Clone)]
pub struct ContainerMetadata {
    pub docker_id: String,
    pub name: String,
    pub image: String,
    pub labels: HashMap<String, String>,
    pub state: String,
    pub status: String,
    pub docker_created_at_ms: Option<i64>,
    pub seen_at_ms: i64,
}

#[derive(Debug, Clone, Serialize, FromRow)]
pub struct MetricSample {
    #[sqlx(default)]
    pub container_id: String,
    pub sampled_at_ms: i64,
    pub cpu_percent: Option<f64>,
    pub memory_working_set_bytes: Option<i64>,
    pub memory_limit_bytes: Option<i64>,
    pub memory_percent: Option<f64>,
    pub network_rx_bytes: Option<i64>,
    pub network_tx_bytes: Option<i64>,
    pub block_read_bytes: Option<i64>,
    pub block_write_bytes: Option<i64>,
    pub pids: Option<i64>,
}

#[derive(Debug, Clone)]
pub struct Observation {
    pub container: ContainerMetadata,
    pub sample: Option<MetricSample>,
}

#[derive(Debug, Clone)]
pub struct NewContainerEvent {
    pub container: ContainerMetadata,
    pub event_type: String,
    pub action: String,
    pub occurred_at_ms: i64,
    pub exit_code: Option<i64>,
    pub oom_killed: bool,
    pub attributes: HashMap<String, String>,
}

#[derive(Debug, Clone, Serialize, FromRow)]
pub struct ContainerSummary {
    pub docker_id: String,
    pub name: String,
    pub image: String,
    pub state: String,
    pub status: String,
    pub first_seen_at_ms: i64,
    pub last_seen_at_ms: i64,
    pub sampled_at_ms: Option<i64>,
    pub cpu_percent: Option<f64>,
    pub memory_working_set_bytes: Option<i64>,
    pub memory_limit_bytes: Option<i64>,
    pub memory_percent: Option<f64>,
    pub network_rx_bytes: Option<i64>,
    pub network_tx_bytes: Option<i64>,
    pub block_read_bytes: Option<i64>,
    pub block_write_bytes: Option<i64>,
    pub pids: Option<i64>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ContainerEvent {
    pub id: i64,
    pub container_id: String,
    pub event_type: String,
    pub action: String,
    pub occurred_at_ms: i64,
    pub exit_code: Option<i64>,
    pub oom_killed: bool,
    pub attributes: HashMap<String, String>,
}

#[derive(Debug, Clone, Serialize, FromRow)]
pub struct CollectorStatus {
    pub docker_connected: bool,
    pub last_success_at_ms: Option<i64>,
    pub last_error: Option<String>,
    pub updated_at_ms: i64,
}

#[derive(Debug, Clone, Serialize)]
pub struct MetricPoint {
    pub sampled_at_ms: i64,
    pub cpu_percent: Option<f64>,
    pub memory_working_set_bytes: Option<f64>,
    pub memory_limit_bytes: Option<f64>,
    pub memory_percent: Option<f64>,
    pub network_rx_bytes_per_second: Option<f64>,
    pub network_tx_bytes_per_second: Option<f64>,
    pub block_read_bytes_per_second: Option<f64>,
    pub block_write_bytes_per_second: Option<f64>,
    pub pids: Option<f64>,
}
