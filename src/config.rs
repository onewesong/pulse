use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct AppConfig {
    pub docker: DockerConfig,
    pub database: DatabaseConfig,
    pub web: WebConfig,
    pub collector: CollectorConfig,
    pub filters: FilterConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct DockerConfig {
    pub socket: String,
    pub timeout_seconds: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct DatabaseConfig {
    pub path: PathBuf,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct WebConfig {
    pub listen: String,
    pub max_points: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct CollectorConfig {
    pub interval_seconds: u64,
    pub retention_days: u64,
    pub concurrency: usize,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct FilterConfig {
    pub include_labels: Vec<String>,
    pub exclude_labels: Vec<String>,
}

impl Default for DockerConfig {
    fn default() -> Self {
        Self {
            socket: "/var/run/docker.sock".into(),
            timeout_seconds: 10,
        }
    }
}

impl Default for DatabaseConfig {
    fn default() -> Self {
        Self {
            path: PathBuf::from("/var/lib/pulse/pulse.db"),
        }
    }
}

impl Default for WebConfig {
    fn default() -> Self {
        Self {
            listen: "127.0.0.1:8080".into(),
            max_points: 2000,
        }
    }
}

impl Default for CollectorConfig {
    fn default() -> Self {
        Self {
            interval_seconds: 15,
            retention_days: 30,
            concurrency: 16,
        }
    }
}

impl AppConfig {
    pub fn load(path: Option<&Path>) -> Result<Self> {
        let mut builder =
            config::Config::builder().add_source(config::Config::try_from(&AppConfig::default())?);

        if let Some(path) = path {
            if !path.exists() {
                bail!("配置文件不存在: {}", path.display());
            }
            builder = builder.add_source(config::File::from(path));
        } else {
            let default_path = Path::new("/etc/pulse/config.toml");
            if default_path.exists() {
                builder = builder.add_source(config::File::from(default_path));
            }
        }

        builder = builder.add_source(
            config::Environment::with_prefix("PULSE")
                .prefix_separator("_")
                .separator("__")
                .try_parsing(true)
                .list_separator(",")
                .with_list_parse_key("filters.include_labels")
                .with_list_parse_key("filters.exclude_labels"),
        );

        let value = builder.build()?.try_deserialize::<Self>()?;
        value.validate()?;
        Ok(value)
    }

    pub fn validate(&self) -> Result<()> {
        if self.collector.interval_seconds == 0 {
            bail!("collector.interval_seconds 必须大于 0");
        }
        if self.collector.retention_days == 0 {
            bail!("collector.retention_days 必须大于 0");
        }
        if self.collector.concurrency == 0 || self.collector.concurrency > 256 {
            bail!("collector.concurrency 必须在 1..=256 之间");
        }
        if !(100..=10_000).contains(&self.web.max_points) {
            bail!("web.max_points 必须在 100..=10000 之间");
        }
        self.web
            .listen
            .parse::<std::net::SocketAddr>()
            .with_context(|| format!("无效的 web.listen: {}", self.web.listen))?;
        if self.docker.socket.trim().is_empty() {
            bail!("docker.socket 不能为空");
        }
        for selector in self
            .filters
            .include_labels
            .iter()
            .chain(&self.filters.exclude_labels)
        {
            validate_selector(selector)?;
        }
        Ok(())
    }
}

pub fn labels_match(
    labels: &std::collections::HashMap<String, String>,
    filters: &FilterConfig,
) -> bool {
    let excluded = filters
        .exclude_labels
        .iter()
        .any(|selector| selector_matches(labels, selector));
    if excluded {
        return false;
    }
    filters.include_labels.is_empty()
        || filters
            .include_labels
            .iter()
            .any(|selector| selector_matches(labels, selector))
}

fn validate_selector(selector: &str) -> Result<()> {
    let key = selector.split_once('=').map_or(selector, |(key, _)| key);
    if key.trim().is_empty() || key.chars().any(char::is_whitespace) {
        bail!("无效的 label 选择器: {selector}");
    }
    Ok(())
}

fn selector_matches(labels: &std::collections::HashMap<String, String>, selector: &str) -> bool {
    match selector.split_once('=') {
        Some((key, value)) => labels.get(key).is_some_and(|actual| actual == value),
        None => labels.contains_key(selector),
    }
}

pub fn ensure_database_parent(path: &Path) -> Result<()> {
    let Some(parent) = path.parent() else {
        return Ok(());
    };
    if !parent.as_os_str().is_empty() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("无法创建数据库目录: {}", parent.display()))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    #[test]
    fn exclude_has_priority_and_include_is_any_match() {
        let labels = HashMap::from([
            ("team".into(), "infra".into()),
            ("pulse.ignore".into(), "true".into()),
        ]);
        let filters = FilterConfig {
            include_labels: vec!["team=infra".into(), "env=prod".into()],
            exclude_labels: vec!["pulse.ignore=true".into()],
        };
        assert!(!labels_match(&labels, &filters));
    }

    #[test]
    fn empty_filters_include_everything() {
        assert!(labels_match(&HashMap::new(), &FilterConfig::default()));
    }
}
