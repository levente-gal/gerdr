-- The tables gerdr reads of T3 Code's database (userdata/statev2.sqlite),
-- as T3 Code 0.1.6 (migration 76) makes them in an empty home. Tests read
-- this snapshot; compare it with a newer version's schema (gerdr saves
-- one that doesn't fit as ~/.local/share/gerdr/t3-schema-<version>.sql).
CREATE TABLE "effect_sql_migrations" (
  migration_id integer PRIMARY KEY NOT NULL,
  created_at datetime NOT NULL DEFAULT current_timestamp,
  name VARCHAR(255) NOT NULL
);
CREATE TABLE projection_projects (
      project_id TEXT PRIMARY KEY,
      title TEXT NOT NULL,
      workspace_root TEXT NOT NULL,
      scripts_json TEXT NOT NULL,
      created_at TEXT NOT NULL,
      updated_at TEXT NOT NULL,
      deleted_at TEXT
    , default_model_selection_json TEXT, default_thread_env_mode TEXT, favicon_path TEXT, auto_pull INTEGER NOT NULL DEFAULT 0, project_icon_json TEXT, pinned_at TEXT, archived_at TEXT);
CREATE INDEX idx_projection_projects_updated_at
    ON projection_projects(updated_at)
  ;
CREATE INDEX idx_projection_projects_workspace_root_deleted_at
    ON projection_projects(workspace_root, deleted_at)
  ;
CREATE TABLE orchestration_v2_projection_threads (
      thread_id TEXT PRIMARY KEY,
      project_id TEXT NOT NULL,
      title TEXT NOT NULL,
      default_provider TEXT NOT NULL,
      runtime_mode TEXT NOT NULL,
      interaction_mode TEXT NOT NULL,
      active_provider_thread_id TEXT,
      created_at TEXT NOT NULL,
      updated_at TEXT NOT NULL,
      archived_at TEXT,
      deleted_at TEXT,
      payload_json TEXT NOT NULL
    , provider_instance_id TEXT);
CREATE INDEX orchestration_v2_projection_threads_project_updated_idx ON orchestration_v2_projection_threads(project_id, updated_at);
CREATE TABLE orchestration_v2_projection_runs (
      run_id TEXT PRIMARY KEY,
      thread_id TEXT NOT NULL,
      ordinal INTEGER NOT NULL,
      provider TEXT NOT NULL,
      provider_thread_id TEXT,
      status TEXT NOT NULL,
      requested_at TEXT NOT NULL,
      completed_at TEXT,
      payload_json TEXT NOT NULL
    , provider_instance_id TEXT);
CREATE UNIQUE INDEX orchestration_v2_projection_runs_thread_ordinal_idx ON orchestration_v2_projection_runs(thread_id, ordinal);
CREATE INDEX orchestration_v2_projection_runs_provider_thread_idx ON orchestration_v2_projection_runs(provider_thread_id);
CREATE INDEX orchestration_v2_projection_runs_thread_status_idx ON orchestration_v2_projection_runs(thread_id, status);
CREATE INDEX orchestration_v2_projection_runs_recovery_idx
    ON orchestration_v2_projection_runs(status, thread_id)
    WHERE status IN ('queued', 'preparing', 'starting', 'running', 'waiting')
  ;
CREATE TABLE orchestration_v2_projection_runtime_requests (
      runtime_request_id TEXT PRIMARY KEY,
      thread_id TEXT NOT NULL,
      node_id TEXT NOT NULL,
      provider_turn_id TEXT,
      kind TEXT NOT NULL,
      status TEXT NOT NULL,
      created_at TEXT NOT NULL,
      resolved_at TEXT,
      payload_json TEXT NOT NULL
    );
CREATE INDEX orchestration_v2_projection_runtime_requests_thread_status_idx ON orchestration_v2_projection_runtime_requests(thread_id, status);
CREATE INDEX orchestration_v2_projection_runtime_requests_provider_turn_idx ON orchestration_v2_projection_runtime_requests(provider_turn_id);
CREATE INDEX orchestration_v2_projection_requests_recovery_idx
    ON orchestration_v2_projection_runtime_requests(thread_id)
    WHERE status = 'pending'
  ;
INSERT INTO effect_sql_migrations (migration_id, name) VALUES (76, 'ProjectionProjectsPinArchive');
