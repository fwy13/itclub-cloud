-- Historical successful jobs seed the counter once. Deleted historical jobs cannot be reconstructed.
CREATE TABLE user_upload_stats (
    user_id TEXT PRIMARY KEY REFERENCES users(id) ON DELETE CASCADE,
    uploaded_bytes INTEGER NOT NULL DEFAULT 0,
    uploaded_files INTEGER NOT NULL DEFAULT 0
);
INSERT INTO user_upload_stats(user_id, uploaded_bytes, uploaded_files)
SELECT owner, COALESCE(SUM(size), 0), COUNT(*) FROM jobs
WHERE status = 'done' AND batch_total = 0 AND node_id IS NOT NULL GROUP BY owner;

-- Count actual completed jobs, not copies or batch parent jobs. Updates are transactional.
CREATE TRIGGER count_completed_upload AFTER UPDATE OF status ON jobs
WHEN NEW.status = 'done' AND OLD.status != 'done' AND NEW.batch_total = 0 AND NEW.node_id IS NOT NULL
BEGIN
    INSERT INTO user_upload_stats(user_id, uploaded_bytes, uploaded_files)
    VALUES(NEW.owner, MAX(NEW.size, 0), 1)
    ON CONFLICT(user_id) DO UPDATE SET
        uploaded_bytes = uploaded_bytes + MAX(NEW.size, 0),
        uploaded_files = uploaded_files + 1;
END;
CREATE TRIGGER count_inserted_upload AFTER INSERT ON jobs
WHEN NEW.status = 'done' AND NEW.batch_total = 0 AND NEW.node_id IS NOT NULL
BEGIN
    INSERT INTO user_upload_stats(user_id, uploaded_bytes, uploaded_files)
    VALUES(NEW.owner, MAX(NEW.size, 0), 1)
    ON CONFLICT(user_id) DO UPDATE SET
        uploaded_bytes = uploaded_bytes + MAX(NEW.size, 0),
        uploaded_files = uploaded_files + 1;
END;
