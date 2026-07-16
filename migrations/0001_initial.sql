CREATE TABLE containers (
    docker_id TEXT PRIMARY KEY NOT NULL,
    name TEXT NOT NULL,
    image TEXT NOT NULL DEFAULT '',
    labels_json TEXT NOT NULL DEFAULT '{}',
    state TEXT NOT NULL DEFAULT 'unknown',
    status TEXT NOT NULL DEFAULT '',
    docker_created_at_ms INTEGER,
    first_seen_at_ms INTEGER NOT NULL,
    last_seen_at_ms INTEGER NOT NULL
);

CREATE INDEX idx_containers_last_seen ON containers(last_seen_at_ms DESC);
CREATE INDEX idx_containers_state ON containers(state);

CREATE TABLE metric_samples (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    container_id TEXT NOT NULL REFERENCES containers(docker_id) ON DELETE CASCADE,
    sampled_at_ms INTEGER NOT NULL,
    cpu_percent REAL,
    memory_working_set_bytes INTEGER,
    memory_limit_bytes INTEGER,
    memory_percent REAL,
    network_rx_bytes INTEGER,
    network_tx_bytes INTEGER,
    block_read_bytes INTEGER,
    block_write_bytes INTEGER,
    pids INTEGER,
    UNIQUE(container_id, sampled_at_ms)
);

CREATE INDEX idx_metric_samples_container_time
    ON metric_samples(container_id, sampled_at_ms DESC);
CREATE INDEX idx_metric_samples_time ON metric_samples(sampled_at_ms);

CREATE TABLE container_events (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    container_id TEXT NOT NULL REFERENCES containers(docker_id) ON DELETE CASCADE,
    event_type TEXT NOT NULL,
    action TEXT NOT NULL,
    occurred_at_ms INTEGER NOT NULL,
    exit_code INTEGER,
    oom_killed INTEGER NOT NULL DEFAULT 0,
    attributes_json TEXT NOT NULL DEFAULT '{}',
    UNIQUE(container_id, event_type, action, occurred_at_ms)
);

CREATE INDEX idx_container_events_container_time
    ON container_events(container_id, occurred_at_ms DESC);
CREATE INDEX idx_container_events_time ON container_events(occurred_at_ms);

CREATE TABLE collector_status (
    id INTEGER PRIMARY KEY CHECK (id = 1),
    docker_connected INTEGER NOT NULL DEFAULT 0,
    last_success_at_ms INTEGER,
    last_error TEXT,
    updated_at_ms INTEGER NOT NULL
);

INSERT INTO collector_status(id, docker_connected, updated_at_ms)
VALUES (1, 0, unixepoch('subsec') * 1000);

