-- The index of AI work (see index.rs): drawn from the events files, never
-- the only copy. A draft; times are milliseconds since the epoch.

-- How far each events file was read.
CREATE TABLE IF NOT EXISTS ingest (
    path TEXT PRIMARY KEY,
    offset INTEGER NOT NULL,
    size INTEGER NOT NULL,
    mtime INTEGER NOT NULL,
    bad_lines INTEGER NOT NULL DEFAULT 0
);

CREATE TABLE IF NOT EXISTS features (
    project TEXT NOT NULL,
    slug TEXT NOT NULL,
    title TEXT NOT NULL DEFAULT '',
    status TEXT NOT NULL DEFAULT '',
    created INTEGER,
    PRIMARY KEY (project, slug)
);

CREATE TABLE IF NOT EXISTS stages (
    project TEXT NOT NULL,
    slug TEXT NOT NULL,
    stage TEXT NOT NULL,
    marked INTEGER,
    PRIMARY KEY (project, slug, stage)
);

CREATE TABLE IF NOT EXISTS runs (
    project TEXT NOT NULL,
    slug TEXT NOT NULL,
    stage TEXT NOT NULL,
    n INTEGER NOT NULL,
    started INTEGER NOT NULL,
    agent TEXT NOT NULL DEFAULT '',
    model TEXT NOT NULL DEFAULT '',
    how TEXT,
    ready INTEGER,
    ended INTEGER,
    why TEXT,
    nudges INTEGER NOT NULL DEFAULT 0,
    send_backs INTEGER NOT NULL DEFAULT 0,
    session TEXT,
    PRIMARY KEY (project, slug, stage, n, started)
);

CREATE TABLE IF NOT EXISTS sends (
    project TEXT NOT NULL,
    slug TEXT NOT NULL,
    stage TEXT NOT NULL,
    n INTEGER NOT NULL,
    at INTEGER NOT NULL,
    kind TEXT NOT NULL,
    via TEXT NOT NULL,
    failed TEXT
);

CREATE TABLE IF NOT EXISTS sessions (
    agent TEXT NOT NULL,
    id TEXT NOT NULL,
    project TEXT NOT NULL,
    slug TEXT NOT NULL,
    raw_bytes INTEGER NOT NULL DEFAULT 0,
    complete INTEGER NOT NULL DEFAULT 0,
    PRIMARY KEY (agent, id)
);

CREATE TABLE IF NOT EXISTS usage (
    agent TEXT NOT NULL,
    id TEXT NOT NULL,
    stage TEXT NOT NULL,
    n INTEGER NOT NULL,
    model TEXT NOT NULL,
    messages INTEGER NOT NULL DEFAULT 0,
    tool_calls INTEGER NOT NULL DEFAULT 0,
    input INTEGER NOT NULL DEFAULT 0,
    output INTEGER NOT NULL DEFAULT 0,
    cache_create INTEGER NOT NULL DEFAULT 0,
    cache_read INTEGER NOT NULL DEFAULT 0,
    PRIMARY KEY (agent, id, stage, n, model)
);

CREATE TABLE IF NOT EXISTS fix_rounds (
    project TEXT NOT NULL,
    slug TEXT NOT NULL,
    n INTEGER NOT NULL,
    opened INTEGER,
    closed INTEGER,
    fixes INTEGER NOT NULL DEFAULT 0,
    PRIMARY KEY (project, slug, n)
);

CREATE TABLE IF NOT EXISTS findings (
    project TEXT NOT NULL,
    slug TEXT NOT NULL,
    review INTEGER NOT NULL,
    severity TEXT NOT NULL,
    count INTEGER NOT NULL DEFAULT 0,
    PRIMARY KEY (project, slug, review, severity)
);

CREATE TABLE IF NOT EXISTS verify_runs (
    project TEXT NOT NULL,
    slug TEXT NOT NULL,
    run TEXT NOT NULL,
    started INTEGER,
    ended INTEGER,
    passed INTEGER,
    PRIMARY KEY (project, slug, run)
);

CREATE TABLE IF NOT EXISTS checks (
    project TEXT NOT NULL,
    slug TEXT NOT NULL,
    run TEXT NOT NULL,
    i INTEGER NOT NULL,
    key TEXT NOT NULL,
    repo TEXT NOT NULL DEFAULT '',
    command TEXT NOT NULL DEFAULT '',
    ended INTEGER,
    secs INTEGER,
    passed INTEGER,
    log TEXT,
    truncated INTEGER NOT NULL DEFAULT 0,
    PRIMARY KEY (project, slug, run, i)
);

CREATE TABLE IF NOT EXISTS setups (
    project TEXT NOT NULL,
    slug TEXT NOT NULL,
    at INTEGER NOT NULL,
    passed INTEGER,
    log TEXT
);

CREATE TABLE IF NOT EXISTS notes (
    project TEXT NOT NULL,
    slug TEXT NOT NULL,
    i INTEGER NOT NULL,
    at INTEGER NOT NULL,
    what TEXT NOT NULL,
    PRIMARY KEY (project, slug, i)
);

CREATE TABLE IF NOT EXISTS manual (
    project TEXT NOT NULL,
    slug TEXT NOT NULL,
    at INTEGER NOT NULL,
    sheet TEXT NOT NULL
);
