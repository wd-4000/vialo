CREATE EXTENSION IF NOT EXISTS btree_gist;

--
-- Bookable assets
--
CREATE TABLE bookable_connectors (
  id int PRIMARY KEY GENERATED ALWAYS AS IDENTITY,
  endpoint text NOT NULL,
  num_outputs int,
  device_name text,
  serial_number text,
  mac macaddr,
  username BYTEA NOT NULL,
  password BYTEA NOT NULL
);

CREATE TABLE bookable_asset_types (
  id int PRIMARY KEY GENERATED ALWAYS AS IDENTITY,
  name_i18n int REFERENCES i18n_index,
  icon citext,
  group_id uuid REFERENCES account_groups (id) ON DELETE SET NULL,
  slug citext UNIQUE
);

CREATE TABLE bookable_assets (
  id int PRIMARY KEY GENERATED ALWAYS AS IDENTITY,
  name_i18n int REFERENCES i18n_index,
  slug citext UNIQUE,
  icon citext,
  asset_type int REFERENCES bookable_asset_types (id) ON DELETE CASCADE,
  quick_unlock tsrange,
  connector int REFERENCES bookable_connectors,
  connector_output_id int
);

CREATE TABLE bookable_schemas (
  id int PRIMARY KEY GENERATED ALWAYS AS IDENTITY,
  label text,
  schedule time[] NOT NULL,
  asset_type int REFERENCES bookable_asset_types (id) ON DELETE CASCADE,
  slot_price int NOT NULL
);

CREATE TABLE bookable_schema_assignments (
  begins timestamptz NOT NULL,
  asset_id int NOT NULL REFERENCES bookable_assets (id) ON DELETE CASCADE,
  schema_id int NOT NULL REFERENCES bookable_schemas (id) ON DELETE CASCADE,
  PRIMARY KEY (begins, asset_id)
);

CREATE TABLE bookable_appointments (
  id uuid PRIMARY KEY default gen_random_uuid(),
  asset_id int NOT NULL REFERENCES bookable_assets (id),
  transaction_id uuid REFERENCES credit_ledger (id),
  account_id uuid NOT NULL REFERENCES accounts (id), -- No cascade here, we want to make sure appointments are refunded appropriately.
  during tsrange NOT NULL,
  activated timestamptz,
  maintenance boolean not null default false,
  CONSTRAINT no_overlapping_appointments_per_asset EXCLUDE USING GIST (
    asset_id
    WITH
      =,
      during
    WITH
      &&
  )
);

CREATE INDEX bookable_appointments_during_idx ON bookable_appointments (during);

CREATE TYPE bookable_status_type AS ENUM(
  'available',
  'quick_unlock',
  'waiting',
  'active',
  'maintenance'
);

CREATE VIEW bookable_asset_status AS
SELECT
  ba.id,
  ba.icon,
  ba.name_i18n,
  ba.asset_type,
  ba.connector,
  ba.connector_output_id,
  COALESCE(
    x.status,
    y.status,
    'available'::bookable_status_type
  ) as status,
  CASE
    WHEN COALESCE(x.during, y.during) IS NOT NULL THEN COALESCE(
      upper(COALESCE(x.during, y.during)),
      'infinity'::timestamp
    )
  END as ends,
  lower(COALESCE(x.during, y.during)) as begins
FROM
  bookable_assets ba
  LEFT JOIN LATERAL (
    SELECT
      ba.quick_unlock AS during,
      'quick_unlock'::bookable_status_type as status
  ) x ON ba.quick_unlock @> now()::timestamp
  LEFT JOIN LATERAL (
    SELECT
      apa.during as during,
      apa.asset_id,
      CASE
        WHEN apa.maintenance THEN 'maintenance'::bookable_status_type
        WHEN apa.activated IS NOT NULL THEN 'active'::bookable_status_type
        ELSE 'waiting'::bookable_status_type
      END AS status
    FROM
      bookable_appointments apa
    WHERE
      apa.asset_id = ba.id
      AND apa.during @> now()::timestamp
    LIMIT
      1
  ) y ON y.asset_id = ba.id;

CREATE OR REPLACE FUNCTION slots_from_schedule (schedule time[], reference_date DATE) RETURNS table (id int, range tsrange) LANGUAGE plpgsql AS $$
    DECLARE
    prel time;
    el time;
    i int = 0;
    BEGIN
        FOREACH el IN ARRAY schedule
        LOOP
            IF prel IS NOT NULL THEN

                RETURN QUERY SELECT i as id, tsrange(
                    (reference_date + prel)::timestamp,
                    (reference_date + el)::timestamp,
                    '[)'
                ) as range;
                i:= i+ 1;
            END IF;

            prel := el;
        END LOOP;
    END
    $$;

CREATE OR REPLACE FUNCTION get_taken_slots (
  p_asset_ids int[],
  p_start_date DATE,
  p_end_date DATE
) RETURNS table (date date, assets jsonb) LANGUAGE plpgsql AS $$
    BEGIN
        RETURN QUERY (SELECT
                j_series_data.j::date as date,
            jsonb_object_agg(sq.asset_id, f.g) as assets
        FROM
            (
                SELECT
                    bsa.asset_id,
                    bsa.schema_id,
                    bsa.begins,
                    LEAD(bsa.begins, 1) OVER (PARTITION BY bsa.asset_id ORDER BY bsa.begins) AS next_begins_date
                FROM
                    bookable_schema_assignments bsa
                WHERE bsa.asset_id = ANY(p_asset_ids)
            ) AS sq,
        LATERAL (
            SELECT *
            FROM generate_series(
                GREATEST(sq.begins, p_start_date),
                LEAST(COALESCE(sq.next_begins_date, p_end_date), p_end_date) - interval '1 day',
                '1 day'::interval
            ) AS j
        ) AS j_series_data,
        LATERAL (SELECT ARRAY(select distinct u.id from (SELECT * FROM slots_from_schedule((SELECT schedule FROM bookable_schemas WHERE id = sq.schema_id), j_series_data.j::date)) u WHERE (SELECT range_agg(ba.during) FROM bookable_appointments ba WHERE ba.asset_id = sq.asset_id AND ba.during && tsrange(j_series_data.j::date + '00:00'::time, j_series_data.j::date + '23:59'::time, '[]')) && u.range) as g) as f WHERE cardinality(f.g) > 0
        GROUP BY j_series_data.j::date, sq.schema_id);
    end;
    $$;
