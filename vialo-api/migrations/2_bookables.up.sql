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
  slug citext UNIQUE
);

CREATE TABLE bookable_assets (
  id int PRIMARY KEY GENERATED ALWAYS AS IDENTITY,
  name_i18n int REFERENCES i18n_index,
  slug citext UNIQUE,
  icon citext,
  asset_type_id int NOT NULL REFERENCES bookable_asset_types (id) ON DELETE CASCADE,
  quick_unlock tstzrange,
  connector int REFERENCES bookable_connectors,
  connector_output_id int
);

-- activation modes:
-- requires_activation=false activation_grace_period=NULL expiry_refund_percent=NULL - auto-activate
-- requires_activation=true activation_grace_period=NULL expiry_refund_percent=NULL - manual activation, no expiry
-- requires_activation=true activation_grace_period=(some amount of time) expiry_refund_percent=(some number) - manual activation, with expiry
CREATE TABLE bookable_schemas (
  id int PRIMARY KEY GENERATED ALWAYS AS IDENTITY,
  label text,
  schedule time[] NOT NULL,
  asset_type_id int NOT NULL REFERENCES bookable_asset_types (id) ON DELETE CASCADE,
  slot_price int NOT NULL,
  requires_activation boolean NOT NULL DEFAULT false,
  activation_grace_period interval,
  expiry_refund_percent smallint CHECK (expiry_refund_percent BETWEEN 0 AND 100),
  CHECK ((activation_grace_period IS NULL) = (expiry_refund_percent IS NULL) -- one doesn't make sense without the other
           AND NOT (NOT requires_activation AND expiry_refund_percent IS NOT NULL)
  )
);

CREATE TABLE bookable_schema_cancellation_policy (
  schema_id int NOT NULL REFERENCES bookable_schemas (id) ON DELETE CASCADE,
  min_notice interval NOT NULL, -- up to min_notice before cancellation,
  refund_percent smallint NOT NULL CHECK (refund_percent BETWEEN 0 AND 100), -- the account will be refunded refund_percent % of the original price
  PRIMARY KEY (schema_id, min_notice)
);

CREATE TABLE bookable_schema_assignments (
  begins date NOT NULL,
  asset_id int NOT NULL REFERENCES bookable_assets (id) ON DELETE CASCADE,
  schema_id int NOT NULL REFERENCES bookable_schemas (id) ON DELETE CASCADE,
  PRIMARY KEY (begins, asset_id)
);

CREATE TYPE bookable_perm AS ENUM('view', 'book', 'admin');

CREATE TABLE bookable_asset_type_group_perms (
  id int PRIMARY KEY GENERATED ALWAYS AS IDENTITY,
  group_id uuid REFERENCES account_groups (id) ON DELETE CASCADE,
  asset_type_id int NOT NULL REFERENCES bookable_asset_types (id) ON DELETE CASCADE,
  perm bookable_perm NOT NULL,
  UNIQUE NULLS NOT DISTINCT (group_id, asset_type_id)
);

CREATE TYPE bookable_appointment_cancellation_reason AS ENUM('expired', 'user', 'admin');

CREATE TABLE bookable_appointments (
  id uuid PRIMARY KEY default gen_random_uuid(),
  asset_id int NOT NULL REFERENCES bookable_assets (id),
  transaction_id uuid REFERENCES credit_ledger (id),
  account_id uuid NOT NULL REFERENCES accounts (id), -- No cascade here, we want to make sure appointments are refunded appropriately.
  during tstzrange NOT NULL,
  activated timestamptz,
  cancelled_at timestamptz,
  cancellation_reason bookable_appointment_cancellation_reason,
  maintenance boolean not null default false,
  CONSTRAINT no_overlapping_appointments_per_asset EXCLUDE USING GIST (
    asset_id
    WITH
      =,
      during
    WITH
      &&
  )
  WHERE
    (cancelled_at IS NULL)
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
  ba.asset_type_id,
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
      'infinity'::timestamptz
    )
  END as ends,
  lower(COALESCE(x.during, y.during)) as begins
FROM
  bookable_assets ba
  LEFT JOIN LATERAL (
    SELECT
      ba.quick_unlock AS during,
      'quick_unlock'::bookable_status_type as status
  ) x ON ba.quick_unlock @> now()
  LEFT JOIN LATERAL (
    SELECT
      apa.during as during,
      apa.asset_id,
      CASE
        WHEN apa.maintenance THEN 'maintenance'::bookable_status_type
        WHEN apa.activated IS NOT NULL
        OR NOT bs.requires_activation THEN 'active'::bookable_status_type
        ELSE 'waiting'::bookable_status_type
      END AS status
    FROM
      bookable_appointments apa
      JOIN bookable_schema_assignments bsa ON bsa.asset_id = apa.asset_id
      AND bsa.begins = (
        SELECT
          MAX(begins)
        FROM
          bookable_schema_assignments
        WHERE
          asset_id = apa.asset_id
          AND begins <= now()
      )
      JOIN bookable_schemas bs ON bs.id = bsa.schema_id
    WHERE
      apa.asset_id = ba.id
      AND apa.during @> now()
      AND apa.cancelled_at IS NULL
    LIMIT
      1
  ) y ON y.asset_id = ba.id;

CREATE OR REPLACE FUNCTION slots_from_schedule (schedule time[], reference_date DATE) RETURNS table (id int, range tstzrange) LANGUAGE plpgsql AS $$
    DECLARE
    prel time;
    el time;
    i int = 0;
    BEGIN
        FOREACH el IN ARRAY schedule
        LOOP
            IF prel IS NOT NULL THEN

                RETURN QUERY SELECT i as id, tstzrange(
                    (reference_date + prel)::timestamptz,
                    (reference_date + el)::timestamptz,
                    '[)'
                ) as range;
                i:= i+ 1;
            END IF;

            prel := el;
        END LOOP;
    END
    $$;

CREATE FUNCTION account_bookable_perm_exists (acid uuid, at_id int, min_perm bookable_perm) RETURNS boolean LANGUAGE sql STABLE AS $$
    SELECT (
        (acid IS NOT NULL AND account_role_exists(acid, 'bookable_manager'))
        OR EXISTS (
            SELECT 1 FROM bookable_asset_type_group_perms
            WHERE asset_type_id = at_id
              AND group_id IS NULL
              AND perm >= min_perm
        )
        OR (acid IS NOT NULL AND EXISTS (
            SELECT 1 FROM account_group_memberships agm
            JOIN bookable_asset_type_group_perms batgp ON agm.group_id = batgp.group_id
            WHERE batgp.asset_type_id = at_id
              AND agm.account_id = acid
              AND batgp.perm >= min_perm
        ))
    )
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
        LATERAL (SELECT ARRAY(select distinct u.id from (SELECT * FROM slots_from_schedule((SELECT schedule FROM bookable_schemas WHERE id = sq.schema_id), j_series_data.j::date)) u WHERE (SELECT range_agg(ba.during) FROM bookable_appointments ba WHERE ba.asset_id = sq.asset_id AND ba.during && tstzrange(j_series_data.j::date + '00:00'::time, j_series_data.j::date + '23:59'::time, '[]')) && u.range) as g) as f WHERE cardinality(f.g) > 0
        GROUP BY j_series_data.j::date, sq.schema_id);
    end;
    $$;
