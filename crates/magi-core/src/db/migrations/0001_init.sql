CREATE TABLE meta (key TEXT PRIMARY KEY, value TEXT NOT NULL);
-- keys: pipeline_version, text_model_id, image_model_id, last_scan_id

CREATE TABLE roots (
  id                INTEGER PRIMARY KEY,
  path              TEXT NOT NULL UNIQUE,          -- canonical absolute path
  enabled           INTEGER NOT NULL DEFAULT 1,
  status            TEXT NOT NULL DEFAULT 'ok',    -- ok|permission_denied|missing|watch_failed
  last_full_scan_at INTEGER,
  added_at          INTEGER NOT NULL
);

CREATE TABLE files (
  id               INTEGER PRIMARY KEY,
  root_id          INTEGER NOT NULL REFERENCES roots(id),
  path             TEXT NOT NULL UNIQUE,
  rel_path         TEXT NOT NULL,
  file_name        TEXT NOT NULL,
  ext              TEXT,
  kind             TEXT NOT NULL,                  -- text|code|pdf|office|image|other
  size             INTEGER NOT NULL,
  mtime_ns         INTEGER NOT NULL,
  content_hash     BLOB,                           -- blake3 (32 bytes)
  lang             TEXT,                           -- ISO 639-1
  state            TEXT NOT NULL,                  -- pending|indexing|indexed|skipped|error
  skip_reason      TEXT,
  error            TEXT,
  attempts         INTEGER NOT NULL DEFAULT 0,
  next_attempt_at  INTEGER,
  pipeline_version INTEGER NOT NULL DEFAULT 0,
  seen_scan_id     INTEGER NOT NULL,
  indexed_at       INTEGER,
  thumb_key        TEXT                            -- content-hash-based cache key
);
CREATE INDEX idx_files_state ON files(state, next_attempt_at);
CREATE INDEX idx_files_root  ON files(root_id);
CREATE INDEX idx_files_hash  ON files(content_hash);

CREATE TABLE chunks (
  id         INTEGER PRIMARY KEY,
  file_id    INTEGER NOT NULL REFERENCES files(id),
  ordinal    INTEGER NOT NULL,
  source     TEXT NOT NULL,        -- body|ocr|qr|filename|code_symbol
  text       TEXT NOT NULL,
  page       INTEGER,
  line_start INTEGER,
  line_end   INTEGER
);
CREATE INDEX idx_chunks_file ON chunks(file_id);

CREATE VIRTUAL TABLE chunks_fts USING fts5(
  text, content = 'chunks', content_rowid = 'id',
  tokenize = 'unicode61 remove_diacritics 2'
);

CREATE TRIGGER chunks_ai AFTER INSERT ON chunks BEGIN
  INSERT INTO chunks_fts(rowid, text) VALUES (new.id, new.text);
END;
CREATE TRIGGER chunks_ad AFTER DELETE ON chunks BEGIN
  INSERT INTO chunks_fts(chunks_fts, rowid, text) VALUES('delete', old.id, old.text);
END;
CREATE TRIGGER chunks_au AFTER UPDATE ON chunks BEGIN
  INSERT INTO chunks_fts(chunks_fts, rowid, text) VALUES('delete', old.id, old.text);
  INSERT INTO chunks_fts(rowid, text) VALUES (new.id, new.text);
END;

CREATE VIRTUAL TABLE vec_text  USING vec0(chunk_id INTEGER PRIMARY KEY, embedding float[384]);
CREATE VIRTUAL TABLE vec_image USING vec0(file_id  INTEGER PRIMARY KEY, embedding float[768]);
