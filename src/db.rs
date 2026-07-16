use std::{path::Path, time::Duration};

use anyhow::{Context, Result};
use sqlx::{
    FromRow, Sqlite, SqlitePool, Transaction,
    sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions, SqliteSynchronous},
};

use crate::models::{
    CollectorStatus, ContainerEvent, ContainerMetadata, ContainerSummary, MetricPoint,
    NewContainerEvent, Observation,
};

#[derive(Clone)]
pub struct Database {
    pool: SqlitePool,
}

#[derive(Debug, FromRow)]
struct AggregatedMetricRow {
    sampled_at_ms: i64,
    cpu_percent: Option<f64>,
    memory_working_set_bytes: Option<f64>,
    memory_limit_bytes: Option<f64>,
    memory_percent: Option<f64>,
    network_rx_bytes: Option<f64>,
    network_tx_bytes: Option<f64>,
    block_read_bytes: Option<f64>,
    block_write_bytes: Option<f64>,
    pids: Option<f64>,
}

#[derive(Debug, FromRow)]
struct ContainerEventRow {
    id: i64,
    container_id: String,
    event_type: String,
    action: String,
    occurred_at_ms: i64,
    exit_code: Option<i64>,
    oom_killed: bool,
    attributes_json: String,
}

impl Database {
    pub async fn connect(path: &Path) -> Result<Self> {
        let options = SqliteConnectOptions::new()
            .filename(path)
            .create_if_missing(true)
            .foreign_keys(true)
            .journal_mode(SqliteJournalMode::Wal)
            .synchronous(SqliteSynchronous::Normal)
            .busy_timeout(Duration::from_secs(5));
        let pool = SqlitePoolOptions::new()
            .max_connections(5)
            .connect_with(options)
            .await
            .with_context(|| format!("无法打开 SQLite 数据库: {}", path.display()))?;
        sqlx::migrate!()
            .run(&pool)
            .await
            .context("数据库迁移失败")?;
        Ok(Self { pool })
    }

    #[cfg(test)]
    pub async fn connect_memory() -> Result<Self> {
        let options = "sqlite::memory:"
            .parse::<SqliteConnectOptions>()?
            .foreign_keys(true);
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect_with(options)
            .await?;
        sqlx::migrate!().run(&pool).await?;
        Ok(Self { pool })
    }

    pub async fn ping(&self) -> Result<()> {
        sqlx::query("SELECT 1").execute(&self.pool).await?;
        Ok(())
    }

    pub async fn write_observations(&self, observations: &[Observation]) -> Result<()> {
        let mut tx = self.pool.begin().await?;
        for observation in observations {
            upsert_container(&mut tx, &observation.container).await?;
            if let Some(sample) = &observation.sample {
                sqlx::query(
                    r#"INSERT INTO metric_samples (
                        container_id, sampled_at_ms, cpu_percent,
                        memory_working_set_bytes, memory_limit_bytes, memory_percent,
                        network_rx_bytes, network_tx_bytes, block_read_bytes,
                        block_write_bytes, pids
                    ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
                    ON CONFLICT(container_id, sampled_at_ms) DO UPDATE SET
                        cpu_percent = excluded.cpu_percent,
                        memory_working_set_bytes = excluded.memory_working_set_bytes,
                        memory_limit_bytes = excluded.memory_limit_bytes,
                        memory_percent = excluded.memory_percent,
                        network_rx_bytes = excluded.network_rx_bytes,
                        network_tx_bytes = excluded.network_tx_bytes,
                        block_read_bytes = excluded.block_read_bytes,
                        block_write_bytes = excluded.block_write_bytes,
                        pids = excluded.pids"#,
                )
                .bind(&sample.container_id)
                .bind(sample.sampled_at_ms)
                .bind(sample.cpu_percent)
                .bind(sample.memory_working_set_bytes)
                .bind(sample.memory_limit_bytes)
                .bind(sample.memory_percent)
                .bind(sample.network_rx_bytes)
                .bind(sample.network_tx_bytes)
                .bind(sample.block_read_bytes)
                .bind(sample.block_write_bytes)
                .bind(sample.pids)
                .execute(&mut *tx)
                .await?;
            }
        }
        tx.commit().await?;
        Ok(())
    }

    pub async fn write_event(&self, event: &NewContainerEvent) -> Result<()> {
        let mut tx = self.pool.begin().await?;
        upsert_container(&mut tx, &event.container).await?;
        sqlx::query(
            r#"INSERT INTO container_events (
                container_id, event_type, action, occurred_at_ms, exit_code,
                oom_killed, attributes_json
            ) VALUES (?, ?, ?, ?, ?, ?, ?)
            ON CONFLICT(container_id, event_type, action, occurred_at_ms) DO NOTHING"#,
        )
        .bind(&event.container.docker_id)
        .bind(&event.event_type)
        .bind(&event.action)
        .bind(event.occurred_at_ms)
        .bind(event.exit_code)
        .bind(event.oom_killed)
        .bind(serde_json::to_string(&event.attributes)?)
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(())
    }

    pub async fn list_containers(&self) -> Result<Vec<ContainerSummary>> {
        let rows = sqlx::query_as::<_, ContainerSummary>(
            r#"SELECT c.docker_id, c.name, c.image, c.state, c.status,
                c.first_seen_at_ms, c.last_seen_at_ms,
                m.sampled_at_ms, m.cpu_percent, m.memory_working_set_bytes,
                m.memory_limit_bytes, m.memory_percent, m.network_rx_bytes,
                m.network_tx_bytes, m.block_read_bytes, m.block_write_bytes, m.pids
            FROM containers c
            LEFT JOIN metric_samples m ON m.id = (
                SELECT id FROM metric_samples
                WHERE container_id = c.docker_id
                ORDER BY sampled_at_ms DESC LIMIT 1
            )
            ORDER BY CASE WHEN c.state = 'running' THEN 0 ELSE 1 END,
                COALESCE(m.cpu_percent, 0) DESC, c.name COLLATE NOCASE"#,
        )
        .fetch_all(&self.pool)
        .await?;
        Ok(rows)
    }

    pub async fn get_container(&self, id: &str) -> Result<Option<ContainerSummary>> {
        let row = sqlx::query_as::<_, ContainerSummary>(
            r#"SELECT c.docker_id, c.name, c.image, c.state, c.status,
                c.first_seen_at_ms, c.last_seen_at_ms,
                m.sampled_at_ms, m.cpu_percent, m.memory_working_set_bytes,
                m.memory_limit_bytes, m.memory_percent, m.network_rx_bytes,
                m.network_tx_bytes, m.block_read_bytes, m.block_write_bytes, m.pids
            FROM containers c
            LEFT JOIN metric_samples m ON m.id = (
                SELECT id FROM metric_samples
                WHERE container_id = c.docker_id
                ORDER BY sampled_at_ms DESC LIMIT 1
            )
            WHERE c.docker_id = ? OR c.docker_id LIKE ?
            ORDER BY length(c.docker_id) LIMIT 1"#,
        )
        .bind(id)
        .bind(format!("{id}%"))
        .fetch_optional(&self.pool)
        .await?;
        Ok(row)
    }

    pub async fn metrics(
        &self,
        container_id: &str,
        from_ms: i64,
        to_ms: i64,
        max_points: usize,
    ) -> Result<Vec<MetricPoint>> {
        let range = (to_ms - from_ms).max(1);
        let bucket_ms = ((range as f64 / max_points.max(1) as f64).ceil() as i64).max(1_000);
        let rows = sqlx::query_as::<_, AggregatedMetricRow>(
            r#"SELECT
                (?1 + CAST((sampled_at_ms - ?1) / ?3 AS INTEGER) * ?3) AS sampled_at_ms,
                AVG(cpu_percent) AS cpu_percent,
                AVG(memory_working_set_bytes) AS memory_working_set_bytes,
                AVG(memory_limit_bytes) AS memory_limit_bytes,
                AVG(memory_percent) AS memory_percent,
                AVG(network_rx_bytes) AS network_rx_bytes,
                AVG(network_tx_bytes) AS network_tx_bytes,
                AVG(block_read_bytes) AS block_read_bytes,
                AVG(block_write_bytes) AS block_write_bytes,
                AVG(pids) AS pids
            FROM metric_samples
            WHERE sampled_at_ms BETWEEN ?1 AND ?2 AND container_id = ?4
            GROUP BY CAST((sampled_at_ms - ?1) / ?3 AS INTEGER)
            ORDER BY sampled_at_ms"#,
        )
        .bind(from_ms)
        .bind(to_ms)
        .bind(bucket_ms)
        .bind(container_id)
        .fetch_all(&self.pool)
        .await?;

        Ok(with_rates(rows))
    }

    pub async fn events(
        &self,
        container_id: &str,
        from_ms: i64,
        to_ms: i64,
    ) -> Result<Vec<ContainerEvent>> {
        let rows = sqlx::query_as::<_, ContainerEventRow>(
            r#"SELECT id, container_id, event_type, action, occurred_at_ms,
                exit_code, oom_killed, attributes_json
            FROM container_events
            WHERE container_id = ? AND occurred_at_ms BETWEEN ? AND ?
            ORDER BY occurred_at_ms"#,
        )
        .bind(container_id)
        .bind(from_ms)
        .bind(to_ms)
        .fetch_all(&self.pool)
        .await?;
        rows.into_iter()
            .map(|row| {
                Ok(ContainerEvent {
                    id: row.id,
                    container_id: row.container_id,
                    event_type: row.event_type,
                    action: row.action,
                    occurred_at_ms: row.occurred_at_ms,
                    exit_code: row.exit_code,
                    oom_killed: row.oom_killed,
                    attributes: serde_json::from_str(&row.attributes_json)?,
                })
            })
            .collect()
    }

    pub async fn set_collector_status(&self, connected: bool, error: Option<&str>) -> Result<()> {
        let now = chrono::Utc::now().timestamp_millis();
        sqlx::query(
            r#"UPDATE collector_status SET
                docker_connected = ?,
                last_success_at_ms = CASE WHEN ? THEN ? ELSE last_success_at_ms END,
                last_error = ?, updated_at_ms = ? WHERE id = 1"#,
        )
        .bind(connected)
        .bind(connected)
        .bind(now)
        .bind(error)
        .bind(now)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn collector_status(&self) -> Result<CollectorStatus> {
        Ok(sqlx::query_as::<_, CollectorStatus>(
            "SELECT docker_connected, last_success_at_ms, last_error, updated_at_ms FROM collector_status WHERE id = 1",
        )
        .fetch_one(&self.pool)
        .await?)
    }

    pub async fn cleanup_before(&self, cutoff_ms: i64) -> Result<(u64, u64, u64)> {
        let mut tx = self.pool.begin().await?;
        let metrics = sqlx::query("DELETE FROM metric_samples WHERE sampled_at_ms < ?")
            .bind(cutoff_ms)
            .execute(&mut *tx)
            .await?
            .rows_affected();
        let events = sqlx::query("DELETE FROM container_events WHERE occurred_at_ms < ?")
            .bind(cutoff_ms)
            .execute(&mut *tx)
            .await?
            .rows_affected();
        let containers =
            sqlx::query("DELETE FROM containers WHERE last_seen_at_ms < ? AND state != 'running'")
                .bind(cutoff_ms)
                .execute(&mut *tx)
                .await?
                .rows_affected();
        tx.commit().await?;
        Ok((metrics, events, containers))
    }
}

async fn upsert_container(
    tx: &mut Transaction<'_, Sqlite>,
    container: &ContainerMetadata,
) -> Result<()> {
    sqlx::query(
        r#"INSERT INTO containers (
            docker_id, name, image, labels_json, state, status,
            docker_created_at_ms, first_seen_at_ms, last_seen_at_ms
        ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)
        ON CONFLICT(docker_id) DO UPDATE SET
            name = excluded.name, image = excluded.image,
            labels_json = excluded.labels_json, state = excluded.state,
            status = excluded.status,
            docker_created_at_ms = COALESCE(excluded.docker_created_at_ms, containers.docker_created_at_ms),
            last_seen_at_ms = excluded.last_seen_at_ms"#,
    )
    .bind(&container.docker_id)
    .bind(&container.name)
    .bind(&container.image)
    .bind(serde_json::to_string(&container.labels)?)
    .bind(&container.state)
    .bind(&container.status)
    .bind(container.docker_created_at_ms)
    .bind(container.seen_at_ms)
    .bind(container.seen_at_ms)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

fn with_rates(rows: Vec<AggregatedMetricRow>) -> Vec<MetricPoint> {
    let mut previous: Option<&AggregatedMetricRow> = None;
    rows.iter()
        .map(|row| {
            let seconds = previous.map(|p| (row.sampled_at_ms - p.sampled_at_ms) as f64 / 1000.0);
            let rate = |current: Option<f64>, prior: Option<f64>| -> Option<f64> {
                let seconds = seconds?;
                let delta = current? - prior?;
                (seconds > 0.0 && delta >= 0.0).then_some(delta / seconds)
            };
            let point = MetricPoint {
                sampled_at_ms: row.sampled_at_ms,
                cpu_percent: row.cpu_percent,
                memory_working_set_bytes: row.memory_working_set_bytes,
                memory_limit_bytes: row.memory_limit_bytes,
                memory_percent: row.memory_percent,
                network_rx_bytes_per_second: rate(
                    row.network_rx_bytes,
                    previous.and_then(|p| p.network_rx_bytes),
                ),
                network_tx_bytes_per_second: rate(
                    row.network_tx_bytes,
                    previous.and_then(|p| p.network_tx_bytes),
                ),
                block_read_bytes_per_second: rate(
                    row.block_read_bytes,
                    previous.and_then(|p| p.block_read_bytes),
                ),
                block_write_bytes_per_second: rate(
                    row.block_write_bytes,
                    previous.and_then(|p| p.block_write_bytes),
                ),
                pids: row.pids,
            };
            previous = Some(row);
            point
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{MetricSample, Observation};
    use std::collections::HashMap;

    fn observation(at: i64, network: i64) -> Observation {
        let id = "abc123".to_string();
        Observation {
            container: ContainerMetadata {
                docker_id: id.clone(),
                name: "demo".into(),
                image: "busybox".into(),
                labels: HashMap::new(),
                state: "running".into(),
                status: "Up".into(),
                docker_created_at_ms: Some(1),
                seen_at_ms: at,
            },
            sample: Some(MetricSample {
                container_id: id,
                sampled_at_ms: at,
                cpu_percent: Some(1.0),
                memory_working_set_bytes: Some(10),
                memory_limit_bytes: Some(100),
                memory_percent: Some(10.0),
                network_rx_bytes: Some(network),
                network_tx_bytes: Some(network),
                block_read_bytes: Some(network),
                block_write_bytes: Some(network),
                pids: Some(1),
            }),
        }
    }

    #[tokio::test]
    async fn writes_queries_and_handles_counter_reset() {
        let db = Database::connect_memory().await.unwrap();
        db.write_observations(&[
            observation(1_000, 100),
            observation(2_000, 200),
            observation(3_000, 5),
        ])
        .await
        .unwrap();
        let points = db.metrics("abc123", 1_000, 3_000, 2000).await.unwrap();
        assert_eq!(points.len(), 3);
        assert_eq!(points[1].network_rx_bytes_per_second, Some(100.0));
        assert_eq!(points[2].network_rx_bytes_per_second, None);
    }

    #[tokio::test]
    async fn cleanup_removes_old_history_and_stopped_container() {
        let db = Database::connect_memory().await.unwrap();
        let mut item = observation(1_000, 1);
        item.container.state = "exited".into();
        db.write_observations(&[item]).await.unwrap();
        let (_, _, containers) = db.cleanup_before(2_000).await.unwrap();
        assert_eq!(containers, 1);
        assert!(db.list_containers().await.unwrap().is_empty());
    }
}
