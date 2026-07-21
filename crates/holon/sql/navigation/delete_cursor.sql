-- Drop a region's cursor row. Used when the last open tab is closed: with no
-- tab left to activate, the cursor-joined main panel query must yield nothing
-- so the region falls through to its default (home) render.
DELETE FROM navigation_cursor WHERE region = $region
