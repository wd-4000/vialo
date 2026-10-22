CREATE EXTENSION IF NOT EXISTS btree_gist;

CREATE TYPE bookable_connector_type AS ENUM('netio', 'miele');

--
-- Bookable assets
--
CREATE TABLE bookable_connectors (
  id int PRIMARY KEY GENERATED ALWAYS AS IDENTITY,
  type bookable_connector_type NOT NULL,
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
  CHECK (
    (activation_grace_period IS NULL) = (expiry_refund_percent IS NULL) -- one doesn't make sense without the other
    AND NOT (
      NOT requires_activation
      AND expiry_refund_percent IS NOT NULL
    )
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

CREATE OR REPLACE FUNCTION enforce_bsa_asset_type () RETURNS trigger AS $$
BEGIN
    IF (SELECT asset_type_id FROM bookable_assets WHERE id = NEW.asset_id)
       <>
       (SELECT asset_type_id FROM bookable_schemas WHERE id = NEW.schema_id) THEN
        RAISE EXCEPTION 'asset_type';
    END IF;
    RETURN NEW;
END;
$$ LANGUAGE plpgsql;

CREATE TRIGGER tg_bsa_asset_type_match
BEFORE INSERT OR UPDATE ON bookable_schema_assignments FOR EACH ROW
EXECUTE FUNCTION enforce_bsa_asset_type ();

CREATE TYPE bookable_perm AS ENUM('view', 'book', 'admin');

CREATE TABLE bookable_asset_type_group_perms (
  id int PRIMARY KEY GENERATED ALWAYS AS IDENTITY,
  group_id uuid REFERENCES account_groups (id) ON DELETE CASCADE,
  asset_type_id int NOT NULL REFERENCES bookable_asset_types (id) ON DELETE CASCADE,
  perm bookable_perm NOT NULL,
  UNIQUE NULLS NOT DISTINCT (group_id, asset_type_id)
);

CREATE TYPE bookable_appointment_cancellation_reason AS ENUM('expired', 'user', 'admin', 'maintenance');

CREATE TYPE bookable_event_kind AS ENUM('booked', 'upcoming', 'canceled', 'expired');

-- Which appointment events a person is notified about
CREATE TABLE bookable_notification_settings (
  account_id uuid PRIMARY KEY REFERENCES accounts_people (id) ON DELETE CASCADE,
  events bookable_event_kind[] NOT NULL,
  upcoming_minutes int NOT NULL CHECK (upcoming_minutes BETWEEN 1 AND 1440)
);

-- Defaults for people who never saved settings
CREATE VIEW bookable_notification_prefs AS
SELECT ap.id AS account_id,
       COALESCE(s.events, '{expired}') AS events,
       COALESCE(s.upcoming_minutes, 15) AS upcoming_minutes
FROM accounts_people ap
LEFT JOIN bookable_notification_settings s ON s.account_id = ap.id;

-- Whether maintenance refunds the appointments it cancels, NULL on regular appointments
CREATE TYPE bookable_maintenance_refund AS ENUM ('refund', 'no_refund');

CREATE TABLE bookable_appointments (
  id uuid PRIMARY KEY default gen_random_uuid(),
  asset_id int NOT NULL REFERENCES bookable_assets (id),
  transaction_id uuid REFERENCES credit_ledger (id),
  account_id uuid REFERENCES accounts (id), -- NULL: held by the asset type's admins. No cascade here, we want to make sure appointments are refunded appropriately.
  during tstzrange NOT NULL,
  created_at timestamptz NOT NULL DEFAULT NOW(),
  activated timestamptz,
  cancelled_at timestamptz,
  cancellation_reason bookable_appointment_cancellation_reason,

  maintenance bookable_maintenance_refund,
  -- NULL: overlapping appointments are canceled when the maintenance is created
  -- else: each one is canceled this long before it begins
  maintenance_refund_rolling_lead interval CHECK (maintenance_refund_rolling_lead >= interval '0'),
  CHECK (maintenance IS NOT NULL OR maintenance_refund_rolling_lead IS NULL),
  CHECK (maintenance IS NULL OR account_id IS NULL),

  reminded_at timestamptz, -- upcoming notification handled (sent or skipped)

  -- Nothing may overlap except rolling maintenance over appointments created before it
  CONSTRAINT no_overlapping_appointments_per_asset EXCLUDE USING GIST (
    asset_id
    WITH
      =,
      during
    WITH
      &&,
      (
        CASE
          WHEN maintenance IS NULL THEN tstzrange(NULL, created_at, '(]')
          WHEN maintenance_refund_rolling_lead IS NULL THEN tstzrange(NULL, NULL)
          ELSE tstzrange(created_at, NULL, '[)')
        END
      )
    WITH
      &&
  )
  WHERE
    (cancelled_at IS NULL)
);

CREATE INDEX bookable_appointments_during_idx ON bookable_appointments (during, id);
CREATE INDEX bookable_appointments_during_gist_idx ON bookable_appointments USING gist (during);
CREATE INDEX bookable_appointments_account_id_during_idx ON bookable_appointments (account_id, during, id);
CREATE INDEX bookable_appointments_transaction_id_idx ON bookable_appointments (transaction_id);

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
  -- an active appointment or maintenance overshadows quick_unlock
  CASE
    WHEN y.status IN ('active', 'maintenance') THEN y.status
    ELSE COALESCE(
      x.status,
      y.status,
      'available'::bookable_status_type
    )
  END as status,
  CASE
    WHEN y.status IN ('active', 'maintenance') THEN upper(y.during)
    WHEN COALESCE(x.during, y.during) IS NOT NULL THEN COALESCE(
      upper(COALESCE(x.during, y.during)),
      'infinity'::timestamptz
    )
  END as ends,
  CASE
    WHEN y.status IN ('active', 'maintenance') THEN lower(y.during)
    ELSE lower(COALESCE(x.during, y.during))
  END as begins,
  y.appointment_id,
  -- maintenance is switched by hand through its activation
  CASE
    WHEN y.status = 'maintenance' THEN y.activated IS NOT NULL
    ELSE COALESCE(x.status, y.status) IN ('active', 'quick_unlock')
  END IS TRUE as powered
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
      apa.id as appointment_id,
      apa.activated,
      CASE
        WHEN apa.maintenance IS NOT NULL THEN 'maintenance'::bookable_status_type
        WHEN apa.activated IS NOT NULL
        OR NOT bs.requires_activation THEN 'active'::bookable_status_type
        ELSE 'waiting'::bookable_status_type
      END AS status
    FROM
      bookable_appointments apa
      LEFT JOIN bookable_schema_assignments bsa ON bsa.asset_id = apa.asset_id
      AND bsa.begins = (
        SELECT
          MAX(begins)
        FROM
          bookable_schema_assignments
        WHERE
          asset_id = apa.asset_id
          AND begins <= now()
      )
      LEFT JOIN bookable_schemas bs ON bs.id = bsa.schema_id
    WHERE
      apa.asset_id = ba.id
      AND apa.during @> now()
      AND apa.cancelled_at IS NULL
    ORDER BY
      apa.maintenance IS NULL
    LIMIT
      1
  ) y ON y.asset_id = ba.id;

-- Who is queued for each asset, split into the three buckets the board shows:
-- finished, running, scheduled. Includes room.
CREATE VIEW bookable_asset_queue AS
WITH q AS (
  SELECT
    apa.asset_id,
    ba.asset_type_id,
    apa.id as appointment_id,
    lower(apa.during) as begins,
    COALESCE(upper(apa.during), 'infinity'::timestamptz) as ends,
    CASE
      WHEN apa.maintenance IS NOT NULL THEN NULL
      ELSE cro.room_name
    END as room,
    apa.maintenance IS NOT NULL as maintenance,
    CASE
      WHEN COALESCE(upper(apa.during), 'infinity'::timestamptz) <= now() THEN 'previous'
      WHEN apa.during @> now() THEN 'current'
      ELSE 'upcoming'
    END as bucket
  FROM
    bookable_appointments apa
    JOIN bookable_assets ba ON ba.id = apa.asset_id
    LEFT JOIN current_room_occupants cro ON cro.id = apa.account_id
  WHERE
    apa.cancelled_at IS NULL
)
SELECT
  q.*,
  -- 1 = most recently finished; caps the previous bucket
  row_number() OVER (
    PARTITION BY
      q.asset_id,
      q.bucket
    ORDER BY
      q.begins DESC
  ) as past_rank,
  -- 1 = soonest within current/upcoming; caps the upcoming bucket
  row_number() OVER (
    PARTITION BY
      q.asset_id,
      q.bucket
    ORDER BY
      q.begins ASC
  ) as future_rank
FROM
  q;

-- Writes to these tables change derived views (bookable_asset_status, bookable_asset_queue), so wake the publisher via NOTIFY.
CREATE OR REPLACE FUNCTION bookable_notify_trigger () RETURNS TRIGGER AS $$
DECLARE
    col text := TG_ARGV[0];
    new_id text;
    old_id text;
BEGIN
    IF TG_OP <> 'DELETE' THEN
        EXECUTE format('SELECT ($1).%I::text', col) INTO new_id USING NEW;
    END IF;
    IF TG_OP <> 'INSERT' THEN
        EXECUTE format('SELECT ($1).%I::text', col) INTO old_id USING OLD;
    END IF;

    IF new_id IS NOT NULL THEN
        PERFORM pg_notify('bookable_update', new_id);
    END IF;
    -- appointment moved between assets
    IF old_id IS NOT NULL AND old_id IS DISTINCT FROM new_id THEN
        PERFORM pg_notify('bookable_update', old_id);
    END IF;

    RETURN NULL;
END;
$$ LANGUAGE plpgsql;

CREATE TRIGGER tg_bookable_update_appointments
AFTER INSERT OR UPDATE OR DELETE ON bookable_appointments FOR EACH ROW
EXECUTE FUNCTION bookable_notify_trigger ('asset_id');

CREATE TRIGGER tg_bookable_update_assets
AFTER INSERT OR UPDATE OR DELETE ON bookable_assets FOR EACH ROW
EXECUTE FUNCTION bookable_notify_trigger ('id');

-- Make sure that any preferences change recomputes the bookable wakeup
CREATE OR REPLACE FUNCTION bookable_reschedule_trigger () RETURNS TRIGGER AS $$
BEGIN
    PERFORM pg_notify('bookable_reschedule', '');
    RETURN NULL;
END;
$$ LANGUAGE plpgsql;

CREATE TRIGGER tg_bookable_reschedule_notification_settings
AFTER INSERT OR UPDATE ON bookable_notification_settings FOR EACH STATEMENT
EXECUTE FUNCTION bookable_reschedule_trigger ();

-- Trigger resync based on connector updates
CREATE TRIGGER tg_bookable_reschedule_connectors
AFTER INSERT OR DELETE OR UPDATE OF type, endpoint, username, password ON bookable_connectors FOR EACH STATEMENT
EXECUTE FUNCTION bookable_reschedule_trigger ();

-- Appointment lifecycle events for notifications. Best-effort: missed if nobody is listening.
CREATE OR REPLACE FUNCTION bookable_event_trigger () RETURNS TRIGGER AS $$
DECLARE
    kind bookable_event_kind;
BEGIN
    -- Only people get notified (maintenance, group and admin-held appointments don't), and imported history must not notify anyone
    IF NOT EXISTS (SELECT 1 FROM accounts_people WHERE id = NEW.account_id) OR current_setting('app.subsystem', true) = 'import' THEN
        RETURN NULL;
    END IF;

    IF TG_OP = 'INSERT' THEN
        kind := 'booked';
    ELSIF OLD.cancelled_at IS NULL AND NEW.cancelled_at IS NOT NULL THEN
        kind := CASE NEW.cancellation_reason WHEN 'expired' THEN 'expired' ELSE 'canceled' END;
    ELSE
        RETURN NULL;
    END IF;

    PERFORM pg_notify('bookable_event', json_build_object('appointment_id', NEW.id, 'kind', kind)::text);
    RETURN NULL;
END;
$$ LANGUAGE plpgsql;

CREATE TRIGGER tg_bookable_event_appointments
AFTER INSERT OR UPDATE OF cancelled_at ON bookable_appointments FOR EACH ROW
EXECUTE FUNCTION bookable_event_trigger ();

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
        LATERAL (SELECT ARRAY(select distinct u.id from (SELECT * FROM slots_from_schedule((SELECT schedule FROM bookable_schemas WHERE id = sq.schema_id), j_series_data.j::date)) u WHERE (SELECT range_agg(ba.during) FROM bookable_appointments ba WHERE ba.asset_id = sq.asset_id AND ba.cancelled_at IS NULL AND ba.during && tstzrange(j_series_data.j::date + '00:00'::time, j_series_data.j::date + '23:59'::time, '[]')) && u.range ORDER BY 1) as g) as f WHERE cardinality(f.g) > 0
        GROUP BY j_series_data.j::date);
    end;
    $$;
