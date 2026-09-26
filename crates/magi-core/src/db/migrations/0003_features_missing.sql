-- ADR-0010: the optional search features a file was indexed without
-- (bit 1 meaning, 2 image_text, 4 image_visual). When a feature starts
-- running, only the indexed files carrying its bit are re-queued.
ALTER TABLE files ADD COLUMN features_missing INTEGER NOT NULL DEFAULT 0;
