-- Id of the newest closed history row that falls OUTSIDE the per-region
-- retention window (the 100 most recent closed rows are kept). Returns no
-- rows while the region is under the cap. Threshold computed here because
-- Turso doesn't support subqueries in DELETE.
SELECT id
FROM navigation_history
WHERE region = $region AND closed_at IS NOT NULL
ORDER BY id DESC
LIMIT 1 OFFSET 100
