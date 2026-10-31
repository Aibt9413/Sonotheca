-- Sonotheca 初始 schema（v1）
-- 约定：时间一律存 Unix 毫秒（INTEGER）；状态/枚举存 TEXT，Rust 侧负责映射。

-- 归档根目录：native = 真实路径（唯一走原子 rename 的模式）
-- saf 仅用于"导出/备份"，不作为下载主目录
CREATE TABLE IF NOT EXISTS library_roots (
  id           INTEGER PRIMARY KEY,
  kind         TEXT    NOT NULL CHECK (kind IN ('native', 'saf')),
  uri          TEXT    NOT NULL,
  display_name TEXT    NOT NULL,
  is_default   INTEGER NOT NULL DEFAULT 0,
  created_at   INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS tracks (
  id             INTEGER PRIMARY KEY,
  root_id        INTEGER NOT NULL REFERENCES library_roots(id) ON DELETE CASCADE,
  rel_path       TEXT    NOT NULL,
  title          TEXT    NOT NULL,
  artist         TEXT    NOT NULL DEFAULT '',
  album          TEXT    NOT NULL DEFAULT '',
  duration_ms    INTEGER,
  bitrate        INTEGER,
  sample_rate    INTEGER,
  channels       INTEGER,
  format         TEXT,
  size_bytes     INTEGER,
  content_hash   TEXT,
  cover_rel_path TEXT,
  path_status    TEXT    NOT NULL DEFAULT 'ok'
                 CHECK (path_status IN ('ok', 'missing', 'moved')),
  added_at       INTEGER NOT NULL,
  updated_at     INTEGER NOT NULL
);
CREATE UNIQUE INDEX IF NOT EXISTS ux_tracks_path    ON tracks(root_id, rel_path);
CREATE INDEX        IF NOT EXISTS ix_tracks_status  ON tracks(path_status);
CREATE INDEX        IF NOT EXISTS ix_tracks_artist  ON tracks(artist, album);

-- 跨源标识：同一首歌在多个源的镜像，以及"灰显"依据
CREATE TABLE IF NOT EXISTS track_sources (
  source          TEXT    NOT NULL,
  source_track_id TEXT    NOT NULL,
  track_id        INTEGER REFERENCES tracks(id) ON DELETE SET NULL,
  availability    TEXT    NOT NULL DEFAULT 'unknown'
                  CHECK (availability IN ('ok', 'no_copyright', 'no_permission', 'unknown')),
  quality_max     TEXT,
  last_seen_at    INTEGER NOT NULL,
  PRIMARY KEY (source, source_track_id)
);
CREATE INDEX IF NOT EXISTS ix_track_sources_track ON track_sources(track_id);

CREATE TABLE IF NOT EXISTS download_tasks (
  id              TEXT PRIMARY KEY,
  source          TEXT NOT NULL,
  source_track_id TEXT NOT NULL,
  title           TEXT,
  artist          TEXT,
  state           TEXT NOT NULL,
  error_kind      TEXT,
  error_msg       TEXT,
  http_status     INTEGER,
  attempts        INTEGER NOT NULL DEFAULT 0,
  bytes_done      INTEGER NOT NULL DEFAULT 0,
  bytes_total     INTEGER,
  snapshot_json   TEXT NOT NULL,
  temp_path       TEXT,
  final_path      TEXT,
  created_at      INTEGER NOT NULL,
  updated_at      INTEGER NOT NULL DEFAULT 0,
  started_at      INTEGER,
  finished_at     INTEGER
);
CREATE INDEX IF NOT EXISTS ix_tasks_state ON download_tasks(state, created_at);
-- 同一首歌在未终结状态下只允许一个任务，避免并发重复下载
CREATE UNIQUE INDEX IF NOT EXISTS ux_tasks_active
  ON download_tasks(source, source_track_id)
  WHERE state NOT IN ('completed', 'failed', 'canceled');

-- 歌单预检与分页结果（无限滚动：只追加，禁止全量拉取）
CREATE TABLE IF NOT EXISTS playlist_cache (
  id         INTEGER PRIMARY KEY,
  source     TEXT    NOT NULL,
  source_id  TEXT    NOT NULL,
  title      TEXT,
  cover_url  TEXT,
  total      INTEGER,
  fetched_at INTEGER NOT NULL,
  UNIQUE (source, source_id)
);

CREATE TABLE IF NOT EXISTS playlist_items (
  playlist_id     INTEGER NOT NULL REFERENCES playlist_cache(id) ON DELETE CASCADE,
  idx             INTEGER NOT NULL,
  source_track_id TEXT    NOT NULL,
  payload_json    TEXT    NOT NULL,
  PRIMARY KEY (playlist_id, idx)
);

CREATE TABLE IF NOT EXISTS settings (
  key        TEXT PRIMARY KEY,
  value_json TEXT NOT NULL
);

-- 最近导入的歌单（M8）：只存元数据，点开再按页拉取
CREATE TABLE IF NOT EXISTS recent_playlists (
  source     TEXT NOT NULL,
  id         TEXT NOT NULL,
  name       TEXT NOT NULL,
  cover_url  TEXT,
  total      INTEGER NOT NULL DEFAULT 0,
  updated_at INTEGER NOT NULL,
  PRIMARY KEY (source, id)
);
CREATE INDEX IF NOT EXISTS ix_recent_playlists_updated
  ON recent_playlists(updated_at DESC);

-- 删除先进 trash，再由 GC 清理；不直接 unlink
CREATE TABLE IF NOT EXISTS trash (
  id                INTEGER PRIMARY KEY,
  root_id           INTEGER,
  original_rel_path TEXT NOT NULL,
  trash_rel_path    TEXT NOT NULL,
  size_bytes        INTEGER,
  deleted_at        INTEGER NOT NULL
);
