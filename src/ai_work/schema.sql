-- The index of AI work (see index.rs): drawn from the events files and the
-- kept chats, never the only copy. Times are milliseconds since the epoch.
-- A feature is (project, slug), as its folder `<project>/<slug>/` names it.

-- How far each file was read: an events file up to `offset`, a chat's
-- transcript by its `size` and `mtime`.
CREATE TABLE IF NOT EXISTS ingest (
    path TEXT PRIMARY KEY,
    offset INTEGER NOT NULL,
    size INTEGER NOT NULL,
    mtime INTEGER NOT NULL,
    bad_lines INTEGER NOT NULL DEFAULT 0
);

-- A feature, from its `record.json` (or its events while that is missing).
CREATE TABLE IF NOT EXISTS features (
    project TEXT NOT NULL,
    slug TEXT NOT NULL,
    dir TEXT NOT NULL,
    title TEXT NOT NULL DEFAULT '',
    status TEXT NOT NULL DEFAULT '',
    created INTEGER,
    PRIMARY KEY (project, slug)
);

-- A stage marked done: when, until its mark is taken back.
CREATE TABLE IF NOT EXISTS stages (
    project TEXT NOT NULL,
    slug TEXT NOT NULL,
    stage TEXT NOT NULL,
    marked INTEGER,
    PRIMARY KEY (project, slug, stage)
);

-- A run of a stage's agent. Run numbers start again after a start over,
-- so `started` is part of what tells one.
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
    nudged INTEGER NOT NULL DEFAULT 0,
    sent_back INTEGER NOT NULL DEFAULT 0,
    machine TEXT,
    held INTEGER NOT NULL DEFAULT 0,
    PRIMARY KEY (project, slug, stage, n, started)
);

-- What changed of a run, at `at`: a run's `RunChange` events.
CREATE TABLE IF NOT EXISTS run_changes (
    project TEXT NOT NULL,
    slug TEXT NOT NULL,
    stage TEXT NOT NULL,
    n INTEGER NOT NULL,
    at INTEGER NOT NULL,
    change TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS run_changes_run ON run_changes (project, slug, stage, n);

-- A line typed into a run's agent; `failed` says why it didn't get there.
CREATE TABLE IF NOT EXISTS sends (
    project TEXT NOT NULL,
    slug TEXT NOT NULL,
    stage TEXT NOT NULL,
    n INTEGER NOT NULL,
    at INTEGER NOT NULL,
    kind TEXT NOT NULL,
    via TEXT NOT NULL,
    failed TEXT,
    -- The start of the SHA-256 of what was typed: a send that failed
    -- after it went is found by it.
    text_sha TEXT NOT NULL DEFAULT ''
);
CREATE INDEX IF NOT EXISTS sends_run ON sends (project, slug, stage, n);

-- A kept chat. `split` tells what its usage was last drawn from.
CREATE TABLE IF NOT EXISTS sessions (
    project TEXT NOT NULL,
    slug TEXT NOT NULL,
    agent TEXT NOT NULL,
    id TEXT NOT NULL,
    raw_bytes INTEGER NOT NULL DEFAULT 0,
    complete INTEGER NOT NULL DEFAULT 0,
    transcript TEXT,
    split TEXT,
    PRIMARY KEY (project, slug, agent, id)
);

-- The runs a chat was of.
CREATE TABLE IF NOT EXISTS session_runs (
    project TEXT NOT NULL,
    slug TEXT NOT NULL,
    agent TEXT NOT NULL,
    id TEXT NOT NULL,
    stage TEXT NOT NULL,
    n INTEGER NOT NULL,
    PRIMARY KEY (project, slug, agent, id, stage, n)
);

-- What a chat cost in each run (`stage`, `n`, `started`), by model, a
-- subagent's apart.
CREATE TABLE IF NOT EXISTS usage (
    project TEXT NOT NULL,
    slug TEXT NOT NULL,
    agent TEXT NOT NULL,
    id TEXT NOT NULL,
    stage TEXT NOT NULL,
    n INTEGER NOT NULL,
    started INTEGER NOT NULL,
    model TEXT NOT NULL,
    sidechain INTEGER NOT NULL DEFAULT 0,
    messages INTEGER NOT NULL DEFAULT 0,
    tool_calls INTEGER NOT NULL DEFAULT 0,
    input INTEGER NOT NULL DEFAULT 0,
    output INTEGER NOT NULL DEFAULT 0,
    cache_create INTEGER NOT NULL DEFAULT 0,
    cache_read INTEGER NOT NULL DEFAULT 0,
    -- When the run's last message of this chat was, in ms; 0 untimed.
    last_at INTEGER NOT NULL DEFAULT 0,
    PRIMARY KEY (project, slug, agent, id, stage, n, started, model, sidechain)
);
CREATE INDEX IF NOT EXISTS usage_run ON usage (project, slug, stage, n, started);

-- A fix round: `fixes` of its `items` made.
CREATE TABLE IF NOT EXISTS fix_rounds (
    project TEXT NOT NULL,
    slug TEXT NOT NULL,
    n INTEGER NOT NULL,
    stage TEXT,
    opened INTEGER,
    closed INTEGER,
    items INTEGER NOT NULL DEFAULT 0,
    fixes INTEGER NOT NULL DEFAULT 0,
    PRIMARY KEY (project, slug, n)
);

-- An AI review ('ai') or plan review ('plan') read in: `count` findings or
-- suggestions.
CREATE TABLE IF NOT EXISTS reviews (
    project TEXT NOT NULL,
    slug TEXT NOT NULL,
    kind TEXT NOT NULL,
    n INTEGER NOT NULL,
    at INTEGER NOT NULL,
    count INTEGER NOT NULL DEFAULT 0,
    PRIMARY KEY (project, slug, kind, n)
);

-- AI review `review`'s findings by severity: from what it wrote when that
-- was kept (`from_blob`), else from the record.
CREATE TABLE IF NOT EXISTS findings (
    project TEXT NOT NULL,
    slug TEXT NOT NULL,
    review INTEGER NOT NULL,
    severity TEXT NOT NULL,
    count INTEGER NOT NULL DEFAULT 0,
    from_blob INTEGER NOT NULL DEFAULT 0,
    PRIMARY KEY (project, slug, review, severity)
);

-- A Verify run, `v{n}@{started}`.
CREATE TABLE IF NOT EXISTS verify_runs (
    project TEXT NOT NULL,
    slug TEXT NOT NULL,
    run TEXT NOT NULL,
    started INTEGER,
    ended INTEGER,
    passed INTEGER,
    PRIMARY KEY (project, slug, run)
);

-- A check of a Verify run; `log` is the path of its kept output.
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
    interrupted INTEGER NOT NULL DEFAULT 0,
    log TEXT,
    truncated INTEGER NOT NULL DEFAULT 0,
    PRIMARY KEY (project, slug, run, i)
);
CREATE INDEX IF NOT EXISTS checks_command ON checks (repo, command);

-- A setup that ended; `log` is the path of its kept output.
CREATE TABLE IF NOT EXISTS setups (
    project TEXT NOT NULL,
    slug TEXT NOT NULL,
    started INTEGER NOT NULL,
    ended INTEGER,
    passed INTEGER,
    log TEXT,
    PRIMARY KEY (project, slug, started)
);

-- A line of the feature's history.
CREATE TABLE IF NOT EXISTS notes (
    project TEXT NOT NULL,
    slug TEXT NOT NULL,
    i INTEGER NOT NULL,
    at INTEGER NOT NULL,
    what TEXT NOT NULL,
    PRIMARY KEY (project, slug, i)
);

-- The manual test sheet as it changed.
CREATE TABLE IF NOT EXISTS manual (
    project TEXT NOT NULL,
    slug TEXT NOT NULL,
    at INTEGER NOT NULL,
    sheet TEXT NOT NULL
);
