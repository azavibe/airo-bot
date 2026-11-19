ALTER TABLE procedures
    ADD COLUMN scored_run_count BIGINT NOT NULL DEFAULT 0 CHECK (scored_run_count >= 0);

UPDATE procedures p
SET scored_run_count = counts.scored
FROM (
    SELECT procedure_id, count(*) FILTER (WHERE outcome IN ('pass', 'fail')) AS scored
    FROM runs
    WHERE deleted_at IS NULL
    GROUP BY procedure_id
) counts
WHERE p.id = counts.procedure_id;
