-- The status thread counts indexed files per root twice a second
-- (`db::files::count_indexed_by_root`): with this index it counts from the
-- index alone, already in root order, instead of reading each row and
-- sorting the groups.
CREATE INDEX idx_files_state_root ON files(state, root_id);
