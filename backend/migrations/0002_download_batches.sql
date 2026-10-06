-- Keep 0001 unchanged so databases created by the first source release migrate.
ALTER TABLE jobs ADD COLUMN module_id TEXT;
ALTER TABLE jobs ADD COLUMN parent_job_id TEXT REFERENCES jobs(id) ON DELETE CASCADE;
ALTER TABLE jobs ADD COLUMN batch_index INTEGER;
ALTER TABLE jobs ADD COLUMN batch_total INTEGER NOT NULL DEFAULT 0;
ALTER TABLE jobs ADD COLUMN batch_done INTEGER NOT NULL DEFAULT 0;
CREATE INDEX job_batch_children ON jobs(parent_job_id, batch_index);
CREATE TABLE remote_sources (
    job_id TEXT PRIMARY KEY REFERENCES jobs(id) ON DELETE CASCADE,
    encrypted_config TEXT NOT NULL,
    staged_complete INTEGER NOT NULL DEFAULT 0
);
