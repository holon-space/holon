-- The definition of every type declared at runtime under the SQL write
-- authority, as TypeDefinition JSON. A boot declares each one again; its rows
-- stay in its `<name>_raw` table.
CREATE TABLE IF NOT EXISTS type_declaration (
    name TEXT PRIMARY KEY NOT NULL,
    definition TEXT NOT NULL
);
