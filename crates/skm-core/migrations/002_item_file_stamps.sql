-- A directory's stamp does not change when its existing metadata file is edited.
ALTER TABLE items ADD COLUMN file_stamp_mtime INTEGER;
ALTER TABLE items ADD COLUMN file_stamp_size INTEGER;
