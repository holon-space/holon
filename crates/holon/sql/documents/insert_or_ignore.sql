INSERT OR IGNORE INTO document (id, parent_id, name, sort_key, properties, created_at, updated_at)
VALUES ($id, $parent_id, $name, $sort_key, $properties, $created_at, $updated_at)