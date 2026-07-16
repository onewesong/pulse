use std::sync::Arc;

use askama::Template;
use axum::{
    Json, Router,
    extract::{Path, Query, State},
    http::{HeaderValue, StatusCode, header},
    response::{Html, IntoResponse, Response},
    routing::get,
};
use serde::{Deserialize, Serialize};
use serde_json::json;
use tower_http::{
    compression::CompressionLayer, set_header::SetResponseHeaderLayer, trace::TraceLayer,
};

use crate::{
    db::Database,
    models::{CollectorStatus, ContainerEvent, ContainerSummary, MetricPoint},
};

#[derive(Clone)]
pub struct AppState {
    pub db: Database,
    pub retention_days: u64,
    pub max_points: usize,
    pub collector_interval_seconds: u64,
}

#[derive(Template)]
#[template(path = "index.html")]
struct DashboardTemplate {
    version: &'static str,
}

#[derive(Debug, Serialize)]
struct ApiErrorBody {
    error: &'static str,
    message: String,
}

#[derive(Debug)]
struct ApiError {
    status: StatusCode,
    code: &'static str,
    message: String,
}

#[derive(Debug, Deserialize)]
struct RangeQuery {
    from: Option<i64>,
    to: Option<i64>,
    max_points: Option<usize>,
}

#[derive(Debug, Deserialize)]
struct AnalysisQuery {
    container_ids: String,
    from: Option<i64>,
    to: Option<i64>,
}

#[derive(Debug, Serialize)]
struct ContainersResponse {
    generated_at_ms: i64,
    containers: Vec<ContainerSummary>,
}

#[derive(Debug, Serialize)]
struct MetricsResponse {
    container: ContainerSummary,
    from_ms: i64,
    to_ms: i64,
    points: Vec<MetricPoint>,
}

#[derive(Debug, Serialize)]
struct EventsResponse {
    container_id: String,
    from_ms: i64,
    to_ms: i64,
    events: Vec<ContainerEvent>,
}

#[derive(Debug, Serialize)]
struct SystemStatusResponse {
    status: &'static str,
    database: &'static str,
    collector: CollectorStatus,
    running_containers: usize,
    stopped_containers: usize,
    collector_interval_seconds: u64,
    retention_days: u64,
    version: &'static str,
}

#[derive(Debug, Serialize)]
struct AnalysisContext {
    schema_version: &'static str,
    generated_at_ms: i64,
    from_ms: i64,
    to_ms: i64,
    containers: Vec<ContainerAnalysis>,
}

#[derive(Debug, Serialize)]
struct ContainerAnalysis {
    container: ContainerSummary,
    statistics: AnalysisStatistics,
    events: Vec<ContainerEvent>,
    series: Vec<MetricPoint>,
}

#[derive(Debug, Default, Serialize)]
struct AnalysisStatistics {
    sample_count: usize,
    cpu_percent: NumericSummary,
    memory_percent: NumericSummary,
    memory_working_set_bytes: NumericSummary,
    network_rx_bytes_per_second: NumericSummary,
    network_tx_bytes_per_second: NumericSummary,
    block_read_bytes_per_second: NumericSummary,
    block_write_bytes_per_second: NumericSummary,
    restart_count: usize,
    oom_count: usize,
    non_zero_exit_count: usize,
}

#[derive(Debug, Default, Serialize)]
struct NumericSummary {
    average: Option<f64>,
    maximum: Option<f64>,
    p95: Option<f64>,
}

impl ApiError {
    fn bad_request(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::BAD_REQUEST,
            code: "invalid_request",
            message: message.into(),
        }
    }

    fn not_found(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::NOT_FOUND,
            code: "not_found",
            message: message.into(),
        }
    }

    fn internal(error: impl std::fmt::Display) -> Self {
        tracing::error!(%error, "API 请求失败");
        Self {
            status: StatusCode::INTERNAL_SERVER_ERROR,
            code: "internal_error",
            message: "服务暂时不可用".into(),
        }
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (
            self.status,
            Json(ApiErrorBody {
                error: self.code,
                message: self.message,
            }),
        )
            .into_response()
    }
}

pub fn router(state: AppState) -> Router {
    let state = Arc::new(state);
    Router::new()
        .route("/", get(index))
        .route("/containers/{id}", get(index))
        .route("/assets/app.js", get(javascript))
        .route("/assets/style.css", get(stylesheet))
        .route("/health/live", get(live))
        .route("/health/ready", get(ready))
        .route("/api/v1/containers", get(containers))
        .route("/api/v1/containers/{id}/metrics", get(metrics))
        .route("/api/v1/containers/{id}/events", get(events))
        .route("/api/v1/analysis/context", get(analysis_context))
        .route("/api/v1/system/status", get(system_status))
        .with_state(state)
        .layer(SetResponseHeaderLayer::if_not_present(
            header::X_CONTENT_TYPE_OPTIONS,
            HeaderValue::from_static("nosniff"),
        ))
        .layer(SetResponseHeaderLayer::if_not_present(
            header::CONTENT_SECURITY_POLICY,
            HeaderValue::from_static("default-src 'self'; script-src 'self'; style-src 'self'; img-src 'self' data:; connect-src 'self'; font-src 'self'"),
        ))
        .layer(CompressionLayer::new())
        .layer(TraceLayer::new_for_http())
}

async fn index() -> Result<Html<String>, ApiError> {
    DashboardTemplate {
        version: env!("CARGO_PKG_VERSION"),
    }
    .render()
    .map(Html)
    .map_err(ApiError::internal)
}

async fn javascript() -> impl IntoResponse {
    (
        [
            (header::CONTENT_TYPE, "text/javascript; charset=utf-8"),
            (header::CACHE_CONTROL, "public, max-age=3600"),
        ],
        include_str!("../web/app.js"),
    )
}

async fn stylesheet() -> impl IntoResponse {
    (
        [
            (header::CONTENT_TYPE, "text/css; charset=utf-8"),
            (header::CACHE_CONTROL, "public, max-age=3600"),
        ],
        include_str!("../web/style.css"),
    )
}

async fn live() -> impl IntoResponse {
    Json(json!({ "status": "ok" }))
}

async fn ready(State(state): State<Arc<AppState>>) -> Result<impl IntoResponse, ApiError> {
    state.db.ping().await.map_err(ApiError::internal)?;
    Ok(Json(json!({ "status": "ready" })))
}

async fn containers(
    State(state): State<Arc<AppState>>,
) -> Result<Json<ContainersResponse>, ApiError> {
    let containers = state
        .db
        .list_containers()
        .await
        .map_err(ApiError::internal)?;
    Ok(Json(ContainersResponse {
        generated_at_ms: now_ms(),
        containers,
    }))
}

async fn metrics(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Query(query): Query<RangeQuery>,
) -> Result<Json<MetricsResponse>, ApiError> {
    let (from, to, max_points) = validated_range(&state, &query)?;
    let container = resolve_container(&state, &id).await?;
    let points = state
        .db
        .metrics(&container.docker_id, from, to, max_points)
        .await
        .map_err(ApiError::internal)?;
    Ok(Json(MetricsResponse {
        container,
        from_ms: from,
        to_ms: to,
        points,
    }))
}

async fn events(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Query(query): Query<RangeQuery>,
) -> Result<Json<EventsResponse>, ApiError> {
    let (from, to, _) = validated_range(&state, &query)?;
    let container = resolve_container(&state, &id).await?;
    let events = state
        .db
        .events(&container.docker_id, from, to)
        .await
        .map_err(ApiError::internal)?;
    Ok(Json(EventsResponse {
        container_id: container.docker_id,
        from_ms: from,
        to_ms: to,
        events,
    }))
}

async fn system_status(
    State(state): State<Arc<AppState>>,
) -> Result<Json<SystemStatusResponse>, ApiError> {
    let collector = state
        .db
        .collector_status()
        .await
        .map_err(ApiError::internal)?;
    let containers = state
        .db
        .list_containers()
        .await
        .map_err(ApiError::internal)?;
    let running_containers = containers
        .iter()
        .filter(|container| container.state == "running")
        .count();
    let stopped_containers = containers.len() - running_containers;
    Ok(Json(SystemStatusResponse {
        status: if collector.docker_connected {
            "ok"
        } else {
            "degraded"
        },
        database: "ok",
        collector,
        running_containers,
        stopped_containers,
        collector_interval_seconds: state.collector_interval_seconds,
        retention_days: state.retention_days,
        version: env!("CARGO_PKG_VERSION"),
    }))
}

async fn analysis_context(
    State(state): State<Arc<AppState>>,
    Query(query): Query<AnalysisQuery>,
) -> Result<Json<AnalysisContext>, ApiError> {
    let range = RangeQuery {
        from: query.from,
        to: query.to,
        max_points: Some(state.max_points.min(1000)),
    };
    let (from, to, max_points) = validated_range(&state, &range)?;
    let ids: Vec<_> = query
        .container_ids
        .split(',')
        .map(str::trim)
        .filter(|id| !id.is_empty())
        .collect();
    if ids.is_empty() || ids.len() > 10 {
        return Err(ApiError::bad_request(
            "container_ids 必须包含 1 到 10 个容器 ID",
        ));
    }

    let mut analyses = Vec::with_capacity(ids.len());
    for id in ids {
        let container = resolve_container(&state, id).await?;
        let series = state
            .db
            .metrics(&container.docker_id, from, to, max_points)
            .await
            .map_err(ApiError::internal)?;
        let events = state
            .db
            .events(&container.docker_id, from, to)
            .await
            .map_err(ApiError::internal)?;
        let statistics = summarize(&series, &events);
        analyses.push(ContainerAnalysis {
            container,
            statistics,
            events,
            series,
        });
    }

    Ok(Json(AnalysisContext {
        schema_version: "1",
        generated_at_ms: now_ms(),
        from_ms: from,
        to_ms: to,
        containers: analyses,
    }))
}

async fn resolve_container(state: &AppState, id: &str) -> Result<ContainerSummary, ApiError> {
    if id.is_empty() || id.len() > 64 || !id.chars().all(|character| character.is_ascii_hexdigit())
    {
        return Err(ApiError::bad_request("容器 ID 格式无效"));
    }
    state
        .db
        .get_container(id)
        .await
        .map_err(ApiError::internal)?
        .ok_or_else(|| ApiError::not_found(format!("未找到容器 {id}")))
}

fn validated_range(state: &AppState, query: &RangeQuery) -> Result<(i64, i64, usize), ApiError> {
    let now = now_ms();
    let to = query.to.unwrap_or(now);
    let from = query.from.unwrap_or_else(|| to.saturating_sub(3_600_000));
    if from < 0 || to <= from {
        return Err(ApiError::bad_request(
            "from 必须早于 to，且时间戳不能为负数",
        ));
    }
    if to > now.saturating_add(300_000) {
        return Err(ApiError::bad_request("to 不能超过当前时间 5 分钟"));
    }
    let max_range = (state.retention_days as i64).saturating_mul(86_400_000);
    if to.saturating_sub(from) > max_range {
        return Err(ApiError::bad_request(format!(
            "查询范围不能超过 {} 天",
            state.retention_days
        )));
    }
    let max_points = query.max_points.unwrap_or(state.max_points);
    if max_points == 0 || max_points > state.max_points {
        return Err(ApiError::bad_request(format!(
            "max_points 必须在 1..={} 之间",
            state.max_points
        )));
    }
    Ok((from, to, max_points))
}

fn summarize(points: &[MetricPoint], events: &[ContainerEvent]) -> AnalysisStatistics {
    AnalysisStatistics {
        sample_count: points.len(),
        cpu_percent: numeric_summary(points.iter().filter_map(|point| point.cpu_percent)),
        memory_percent: numeric_summary(points.iter().filter_map(|point| point.memory_percent)),
        memory_working_set_bytes: numeric_summary(
            points
                .iter()
                .filter_map(|point| point.memory_working_set_bytes),
        ),
        network_rx_bytes_per_second: numeric_summary(
            points
                .iter()
                .filter_map(|point| point.network_rx_bytes_per_second),
        ),
        network_tx_bytes_per_second: numeric_summary(
            points
                .iter()
                .filter_map(|point| point.network_tx_bytes_per_second),
        ),
        block_read_bytes_per_second: numeric_summary(
            points
                .iter()
                .filter_map(|point| point.block_read_bytes_per_second),
        ),
        block_write_bytes_per_second: numeric_summary(
            points
                .iter()
                .filter_map(|point| point.block_write_bytes_per_second),
        ),
        restart_count: events
            .iter()
            .filter(|event| event.action == "restart")
            .count(),
        oom_count: events
            .iter()
            .filter(|event| event.oom_killed || event.action == "oom")
            .count(),
        non_zero_exit_count: events
            .iter()
            .filter(|event| event.exit_code.is_some_and(|code| code != 0))
            .count(),
    }
}

fn numeric_summary(values: impl Iterator<Item = f64>) -> NumericSummary {
    let mut values: Vec<f64> = values.filter(|value| value.is_finite()).collect();
    if values.is_empty() {
        return NumericSummary::default();
    }
    values.sort_by(f64::total_cmp);
    let average = values.iter().sum::<f64>() / values.len() as f64;
    let index = ((values.len() - 1) as f64 * 0.95).ceil() as usize;
    NumericSummary {
        average: Some(average),
        maximum: values.last().copied(),
        p95: values.get(index).copied(),
    }
}

fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use axum::{
        body::{Body, to_bytes},
        http::Request,
    };
    use tower::ServiceExt;

    use crate::models::{ContainerMetadata, MetricSample, Observation};

    use super::*;

    #[test]
    fn numeric_summary_calculates_average_max_and_p95() {
        let summary = numeric_summary((1..=100).map(f64::from));
        assert_eq!(summary.average, Some(50.5));
        assert_eq!(summary.maximum, Some(100.0));
        assert_eq!(summary.p95, Some(96.0));
    }

    #[tokio::test]
    async fn validates_time_range_and_point_limit() {
        let state = AppState {
            db: Database::connect_memory().await.unwrap(),
            retention_days: 30,
            max_points: 2000,
            collector_interval_seconds: 15,
        };
        let now = now_ms();
        assert!(
            validated_range(
                &state,
                &RangeQuery {
                    from: Some(now - 1_000),
                    to: Some(now),
                    max_points: Some(2001)
                }
            )
            .is_err()
        );
        assert!(
            validated_range(
                &state,
                &RangeQuery {
                    from: Some(now),
                    to: Some(now - 1),
                    max_points: None
                }
            )
            .is_err()
        );
    }

    #[tokio::test]
    async fn dashboard_and_empty_container_api_are_available_offline() {
        let app = test_router().await;
        let response = app
            .clone()
            .oneshot(Request::builder().uri("/").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = String::from_utf8(
            to_bytes(response.into_body(), usize::MAX)
                .await
                .unwrap()
                .to_vec(),
        )
        .unwrap();
        assert!(body.contains("Pulse"));
        assert!(!body.contains("https://"));

        let response = app
            .oneshot(
                Request::builder()
                    .uri("/api/v1/containers")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["containers"].as_array().unwrap().len(), 0);
    }

    #[tokio::test]
    async fn metrics_reject_invalid_ranges_and_unknown_containers() {
        let app = test_router().await;
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/api/v1/containers/abc/metrics?from=2000&to=1000")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);

        let response = app
            .oneshot(
                Request::builder()
                    .uri("/api/v1/containers/abc/metrics?from=1000&to=2000")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn analysis_context_has_versioned_summary_and_series() {
        let db = Database::connect_memory().await.unwrap();
        let now = now_ms();
        let id = "a".repeat(64);
        db.write_observations(&[Observation {
            container: ContainerMetadata {
                docker_id: id.clone(),
                name: "demo".into(),
                image: "busybox".into(),
                labels: HashMap::new(),
                state: "running".into(),
                status: "Up".into(),
                docker_created_at_ms: Some(now - 10_000),
                seen_at_ms: now,
            },
            sample: Some(MetricSample {
                container_id: id.clone(),
                sampled_at_ms: now - 1_000,
                cpu_percent: Some(12.5),
                memory_working_set_bytes: Some(100),
                memory_limit_bytes: Some(1000),
                memory_percent: Some(10.0),
                network_rx_bytes: Some(10),
                network_tx_bytes: Some(20),
                block_read_bytes: Some(30),
                block_write_bytes: Some(40),
                pids: Some(2),
            }),
        }])
        .await
        .unwrap();
        let app = router(AppState {
            db,
            retention_days: 30,
            max_points: 2000,
            collector_interval_seconds: 15,
        });
        let uri = format!(
            "/api/v1/analysis/context?container_ids={id}&from={}&to={now}",
            now - 5_000
        );
        let response = app
            .oneshot(Request::builder().uri(uri).body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["schema_version"], "1");
        assert_eq!(json["containers"][0]["statistics"]["sample_count"], 1);
    }

    async fn test_router() -> Router {
        router(AppState {
            db: Database::connect_memory().await.unwrap(),
            retention_days: 30,
            max_points: 2000,
            collector_interval_seconds: 15,
        })
    }
}
