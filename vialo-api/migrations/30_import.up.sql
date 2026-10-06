--
-- Import
--
-- Which row an externally imported record became
CREATE TABLE import_refs (
  source text NOT NULL, -- the legacy database name
  kind text NOT NULL, -- the legacy table name
  source_key text NOT NULL,
  target text NOT NULL, -- PK of the created row
  created_at timestamptz NOT NULL DEFAULT NOW(),
  PRIMARY KEY (source, kind, source_key)
);
