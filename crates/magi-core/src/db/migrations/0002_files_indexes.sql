-- The move lookup (`db::files::hashed_of_size`) runs once per new file in a
-- scan; without this it read all of `files` each time.
CREATE INDEX idx_files_size ON files(size, kind) WHERE content_hash IS NOT NULL;

-- The scheduler reads the newest pending rows on every poll
-- (`db::files::next_pending`): walk them in order instead of sorting them all.
CREATE INDEX idx_files_pending ON files(mtime_ns DESC, id) WHERE state = 'pending';
