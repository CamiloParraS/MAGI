-- Why a file failed, as a stable code the UI localizes (SPEC.md §5.7
-- locale neutrality). `error` keeps the diagnostic text. Set exactly when
-- `error` is; rows that failed before this migration get `other`.
ALTER TABLE files ADD COLUMN error_code TEXT;
UPDATE files SET error_code = 'other' WHERE error IS NOT NULL;
