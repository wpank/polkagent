-- V10: Add deadline_at column to runs table for persisting per-run timeout
-- deadlines across process restarts.
--
-- Previously, run deadlines were computed from Instant::now() at process start,
-- which is process-local and lost on crash/restart. This column stores the
-- absolute wall-clock deadline so it can be restored during recovery.

ALTER TABLE runs ADD COLUMN deadline_at TEXT;  -- ISO 8601, NULL if no timeout
