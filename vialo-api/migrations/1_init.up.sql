-- Accounts, Groups and Credits
CREATE EXTENSION citext;
CREATE EXTENSION IF NOT EXISTS pg_stat_statements; -- needs shared_preload_libraries, see vialo/compose.yaml

--
-- Internationalized strings
--
-- This table is here just for autoincrement
CREATE TABLE i18n_index (id int PRIMARY KEY GENERATED ALWAYS AS IDENTITY);

CREATE TABLE i18n_langs (
  id int PRIMARY KEY GENERATED ALWAYS AS IDENTITY,
  label text NOT NULL
);

CREATE TABLE i18n_strings (
  id int NOT NULL REFERENCES i18n_index (id) ON DELETE CASCADE,
  lang int NOT NULL REFERENCES i18n_langs (id) ON DELETE CASCADE,
  content text NOT NULL,
  PRIMARY KEY (id, lang)
);

CREATE OR REPLACE FUNCTION get_i18n_string (
  i18n_id INT, -- The ID of the translation entry in i18n_index
  lang_priority TEXT[] -- Array of language labels, starting from the first choice
) RETURNS TEXT LANGUAGE plpgsql AS $$
DECLARE
    result TEXT;
    lang_id INT;
    lang_label TEXT;
    i INT;
BEGIN
    -- Loop over the language priority array
    FOR i IN 1..array_length(lang_priority, 1) LOOP
        lang_label := lang_priority[i];

        -- Get the language ID from the language label
        SELECT id
        INTO lang_id
        FROM i18n_langs
        WHERE label = lang_label
        LIMIT 1;

        -- If no language with that label exists, skip to the next one
        IF NOT FOUND THEN
            CONTINUE;
        END IF;

        -- Try to fetch the translation for the current language
        SELECT content
        INTO result
        FROM i18n_strings
        WHERE id = i18n_id AND lang = lang_id
        LIMIT 1;

        -- If we found a translation, exit the loop
        IF result IS NOT NULL THEN
            RETURN result;
        END IF;
    END LOOP;

    -- If no translation was found, return NULL
    RETURN NULL;
END;
$$;

CREATE OR REPLACE FUNCTION get_i18n_all_string_translations (i18n_id INT) RETURNS JSONB AS $$
BEGIN
    RETURN (
        SELECT jsonb_object_agg(lg.label, content)
        FROM i18n_strings st
        JOIN i18n_langs lg ON lg.id = st.lang
        WHERE st.id = i18n_id
        GROUP BY st.id
    );
END;
$$ LANGUAGE plpgsql;

CREATE OR REPLACE FUNCTION update_i18n_strings (
  idx_id int,
  language_labels text[], -- Array of language labels
  contents text[] -- Array of contents corresponding to the languages
) RETURNS int LANGUAGE plpgsql AS $$
DECLARE
    lang_id int;
    i int;
BEGIN
    -- Check that both arrays have the same length
    IF array_length(language_labels, 1) != array_length(contents, 1) THEN
        RAISE EXCEPTION 'Arrays must have the same length';
    END IF;

    IF idx_id IS NULL THEN
        INSERT INTO i18n_index DEFAULT VALUES
        RETURNING id INTO idx_id;
    END IF;

    -- Loop through the arrays
    FOR i IN 1..array_length(language_labels, 1) LOOP
        -- Check if the language already exists, otherwise insert
        SELECT id INTO lang_id
        FROM i18n_langs
        WHERE label = language_labels[i]
        LIMIT 1;

        IF NOT FOUND THEN
            -- If language doesn't exist, insert and get the new lang_id
            INSERT INTO i18n_langs (label)
            VALUES (language_labels[i])
            RETURNING id INTO lang_id;
        END IF;

        -- Insert the content into i18n_strings
        INSERT INTO i18n_strings (id, lang, content)
        VALUES (idx_id, lang_id, contents[i])
        ON CONFLICT (id, lang) DO UPDATE SET content = contents[i];

    END LOOP;
    RETURN idx_id;
END;
$$;

CREATE OR REPLACE FUNCTION insert_i18n_strings (
  language_labels text[], -- Array of language labels
  contents text[] -- Array of contents corresponding to the languages
) RETURNS int LANGUAGE plpgsql AS $$
DECLARE
    lang_id int;
    idx_id int;
    i int;
BEGIN
    -- Check that both arrays have the same length
    IF array_length(language_labels, 1) != array_length(contents, 1) THEN
        RAISE EXCEPTION 'Arrays must have the same length';
    END IF;

   -- Insert the content into the i18n_index table
   INSERT INTO i18n_index DEFAULT VALUES
   RETURNING id INTO idx_id;

    -- Loop through the arrays
    FOR i IN 1..array_length(language_labels, 1) LOOP
        -- Check if the language already exists, otherwise insert
        SELECT id INTO lang_id
        FROM i18n_langs
        WHERE label = language_labels[i]
        LIMIT 1;

        IF NOT FOUND THEN
            -- If language doesn't exist, insert and get the new lang_id
            INSERT INTO i18n_langs (label)
            VALUES (language_labels[i])
            RETURNING id INTO lang_id;
        END IF;

        -- Insert the content into i18n_strings
        INSERT INTO i18n_strings (id, lang, content)
        VALUES (idx_id, lang_id, contents[i]);

    END LOOP;
    RETURN idx_id;
END;
$$;


--
-- Accounts
--
-- Table that stores Ory Kratos identity info
CREATE TABLE identities (
  id uuid PRIMARY KEY,
  full_name text,
  display_name text,
  email text,
  phone text,
  room text
);

-- General account info
CREATE TABLE accounts (
  id uuid PRIMARY KEY default gen_random_uuid(),
  full_name text
);

CREATE TABLE accounts_people (
  id uuid PRIMARY KEY REFERENCES accounts (id) ON DELETE CASCADE,
  label text, -- Account purpose (or something else in the future)
  auth_id uuid REFERENCES identities (id), -- Ory Kratos ID. NULL = Either a virtual user, or just not signed up yet.
  email citext UNIQUE,
  membership_start date,
  membership_end date not null, -- Association membership end. Expired = No access.
  comms_lang text,
  full_name text,
  display_name text,
  phone text,
  manually_suspended boolean not null default false, -- True = No access to any syiervices.
  credit_balance int DEFAULT 0, -- NULL = unlimited credits
  amenities_username text UNIQUE, -- Amenities login username. NULL = not provisioned.
  amenities_pin BYTEA -- Encrypted amenities login PIN
);

CREATE INDEX account_auth_idx ON accounts_people (auth_id);

-- Account groups
CREATE TABLE account_groups (
  id uuid PRIMARY KEY REFERENCES accounts (id),
  description_i18n int REFERENCES i18n_index,
  label text NOT NULL, -- Group name
  icon text,
  email citext UNIQUE,
  public bool NOT NULL,
  index int NOT NULL DEFAULT 0 -- display order
);

CREATE TYPE group_role AS ENUM('member', 'manager');

CREATE TABLE account_group_memberships_manual (
  account_id uuid NOT NULL REFERENCES accounts_people (id) ON DELETE CASCADE,
  group_id uuid NOT NULL REFERENCES account_groups (id),
  role group_role NOT NULL,
  PRIMARY KEY (account_id, group_id)
);

CREATE INDEX account_group_memberships_manual_group_id_idx ON account_group_memberships_manual (group_id);

CREATE TYPE group_source_type AS ENUM('floor', 'group_id', 'group_type');

-- Rules that put people into a group automatically
CREATE TABLE account_group_source_rules (
  id int PRIMARY KEY GENERATED ALWAYS AS IDENTITY,
  group_id uuid NOT NULL REFERENCES account_groups (id) ON DELETE CASCADE,
  type group_source_type NOT NULL,
  floor int, -- type=floor: current tenants of this floor
  source_group_id uuid REFERENCES account_groups (id) ON DELETE CASCADE, -- type=group_id: members of this group
  source_group_type group_source_type, -- type=group_type: members of every group that has a rule of this type
  source_role group_role, -- type=group_id/group_type: only members with this role in the source group. NULL=any role
  CHECK ((type = 'floor') = (floor IS NOT NULL)),
  CHECK ((type = 'group_id') = (source_group_id IS NOT NULL)),
  CHECK ((type = 'group_type') = (source_group_type IS NOT NULL)),
  CHECK (type != 'floor' OR source_role IS NULL),
  CHECK (source_group_type = 'floor') -- keep rules one level deep
);

CREATE INDEX account_group_source_rules_group_id_idx ON account_group_source_rules (group_id);

-- A group's rules in the API shape (GroupSourceRule)
CREATE OR REPLACE FUNCTION get_group_source_rules (gid uuid) RETURNS jsonb[] LANGUAGE sql STABLE AS $$
    SELECT COALESCE(array_agg(jsonb_strip_nulls(jsonb_build_object(
        'type', type,
        'floor', floor,
        'group_id', source_group_id,
        'group_type', source_group_type,
        'role', source_role
    )) ORDER BY id), '{}')
    FROM account_group_source_rules
    WHERE group_id = gid
$$;

-- Materialized from account_group_source_rules by refresh_derived_memberships()
CREATE TABLE account_group_memberships_derived (
  account_id uuid NOT NULL REFERENCES accounts_people (id) ON DELETE CASCADE,
  group_id uuid NOT NULL REFERENCES account_groups (id) ON DELETE CASCADE,
  source group_source_type NOT NULL, -- Type of the rule that produced this row; earliest enum value wins if several apply
  PRIMARY KEY (account_id, group_id)
);

CREATE INDEX account_group_memberships_derived_group_id_idx ON account_group_memberships_derived (group_id);

-- Manual rows take precedence over derived ones
CREATE VIEW account_group_memberships AS
SELECT
  account_id,
  group_id,
  role
FROM
  account_group_memberships_manual
UNION ALL
SELECT
  d.account_id,
  d.group_id,
  'member'::group_role
FROM
  account_group_memberships_derived d
WHERE
  NOT EXISTS (
    SELECT
      1
    FROM
      account_group_memberships_manual m
    WHERE
      m.account_id = d.account_id
      AND m.group_id = d.group_id
  );

CREATE OR REPLACE FUNCTION enforce_account_class () RETURNS TRIGGER AS $$
DECLARE
    f_account_id uuid;
    f_full_name text;
BEGIN
    IF (TG_OP = 'UPDATE') THEN
        IF TG_TABLE_NAME = 'accounts_people' THEN
            f_full_name := COALESCE(NEW.full_name, NEW.label);
        ELSE -- account_groups
            f_full_name := NEW.label;
        END IF;

        UPDATE accounts SET full_name = f_full_name WHERE id = NEW.id;
        RETURN NEW;
    ELSIF (TG_OP = 'INSERT') THEN
        IF TG_TABLE_NAME = 'accounts_people' THEN
            f_full_name := COALESCE(NEW.full_name, NEW.label);
        ELSE -- account_groups
            f_full_name := NEW.label;
        END IF;

        insert into accounts (id, full_name) VALUES (COALESCE(NEW.id, gen_random_uuid()), f_full_name) RETURNING id INTO f_account_id;
        NEW.id := f_account_id;

        RETURN NEW;
    ELSIF TG_OP = 'DELETE' THEN
        DELETE FROM accounts WHERE id = OLD.id;
        RETURN OLD;
    END IF;

END;
$$ LANGUAGE plpgsql;

create trigger tg_account_class
before insert or update or delete on accounts_people for each row
execute FUNCTION enforce_account_class ();

create trigger tg_account_class
before insert or update or delete on account_groups for each row
execute FUNCTION enforce_account_class ();

CREATE TYPE app_role AS ENUM(
  'account_manager',
  'credit_manager',
  'bookable_manager',
  'board_manager',
  'home_manager',
  'network_manager',
  'settings_manager',
  'history_viewer',
  'health_viewer'
);

CREATE TABLE account_group_app_roles (
  role app_role,
  group_id uuid REFERENCES account_groups (id),
  PRIMARY KEY (group_id, role)
);

CREATE OR REPLACE FUNCTION account_role_exists (acid uuid, rl app_role) RETURNS BOOLEAN LANGUAGE sql STABLE STRICT AS $$
    SELECT COUNT(*) > 0
    FROM account_group_memberships agm
    JOIN account_group_app_roles agra
    ON agm.group_id = agra.group_id
    WHERE agm.account_id = acid AND agra.role = rl
$$;

--
-- Residents
--
-- Rooms
CREATE TABLE res_rooms (
  id uuid PRIMARY KEY default gen_random_uuid(),
  label text UNIQUE NOT NULL,
  capacity int CHECK (capacity > 0),
  vlan int,
  floor int
);

-- Leases/contracts for the rooms
CREATE TABLE res_leases (
  id uuid PRIMARY KEY default gen_random_uuid(),
  tenant_id uuid NOT NULL REFERENCES accounts_people (id) ON DELETE CASCADE UNIQUE, -- We don't care about old records, so this is unique
  lessor_id uuid REFERENCES accounts_people (id), -- NULL means StW. Cascade has to be implemented in-app for the case an active contract is attempted to be deleted.
  room_id uuid NOT NULL REFERENCES res_rooms (id) ON DELETE CASCADE, -- If a room stops existing there isn't much sense in still having contracts for it.
  begins date NOT NULL,
  ends date NOT NULL
);

CREATE INDEX res_leases_room_id_idx ON res_leases (room_id);

CREATE INDEX res_rooms_floor_idx ON res_rooms (floor);

-- Rebuild account_group_memberships_derived
-- Run by triggers, but because of leases beginning/expiring we also need a scheduled task
CREATE OR REPLACE FUNCTION refresh_derived_memberships () RETURNS void LANGUAGE plpgsql AS $$
BEGIN
    PERFORM pg_advisory_xact_lock(hashtext('refresh_derived_memberships'));

    WITH floor_level AS (
        SELECT l.tenant_id AS account_id, s.group_id
        FROM account_group_source_rules s
        JOIN res_rooms r ON r.floor = s.floor
        JOIN res_leases l ON l.room_id = r.id
        WHERE s.type = 'floor' AND l.begins <= CURRENT_DATE AND l.ends >= CURRENT_DATE
    ),
    base AS (
        SELECT account_id, group_id, role FROM account_group_memberships_manual
        UNION ALL
        SELECT account_id, group_id, 'member'::group_role FROM floor_level
    ),
    -- (target group, rule type, role filter, group to read members from) for each group_id/group_type rule
    rule_sources AS (
        SELECT s.group_id, s.type, s.source_role, s.source_group_id AS from_group
        FROM account_group_source_rules s
        WHERE s.type = 'group_id'
        UNION
        SELECT s.group_id, s.type, s.source_role, s2.group_id
        FROM account_group_source_rules s
        JOIN account_group_source_rules s2 ON s2.type = s.source_group_type
        WHERE s.type = 'group_type'
    ),
    wanted AS (
        SELECT account_id, group_id, min(source) AS source
        FROM (
            SELECT account_id, group_id, 'floor'::group_source_type AS source FROM floor_level
            UNION ALL
            SELECT b.account_id, rs.group_id, rs.type
            FROM rule_sources rs
            JOIN base b ON b.group_id = rs.from_group
            WHERE rs.source_role IS NULL OR b.role = rs.source_role
        ) w
        GROUP BY account_id, group_id
    ),
    del AS (
        DELETE FROM account_group_memberships_derived d
        WHERE NOT EXISTS (SELECT 1 FROM wanted w WHERE w.account_id = d.account_id AND w.group_id = d.group_id)
    )
    INSERT INTO account_group_memberships_derived AS d (account_id, group_id, source)
    SELECT account_id, group_id, source FROM wanted
    ON CONFLICT (account_id, group_id) DO UPDATE SET source = EXCLUDED.source
    WHERE d.source IS DISTINCT FROM EXCLUDED.source;
END;
$$;

CREATE OR REPLACE FUNCTION refresh_derived_memberships_on_change () RETURNS TRIGGER AS $$
BEGIN
    PERFORM refresh_derived_memberships();
    RETURN NULL;
END;
$$ LANGUAGE plpgsql;

CREATE TRIGGER tg_refresh_derived_memberships
AFTER INSERT OR UPDATE OR DELETE ON res_leases FOR EACH STATEMENT
EXECUTE FUNCTION refresh_derived_memberships_on_change ();

CREATE TRIGGER tg_refresh_derived_memberships
AFTER INSERT OR UPDATE OR DELETE ON res_rooms FOR EACH STATEMENT
EXECUTE FUNCTION refresh_derived_memberships_on_change ();

CREATE TRIGGER tg_refresh_derived_memberships
AFTER INSERT OR UPDATE OR DELETE ON account_group_source_rules FOR EACH STATEMENT
EXECUTE FUNCTION refresh_derived_memberships_on_change ();

CREATE TRIGGER tg_refresh_derived_memberships
AFTER INSERT OR UPDATE OR DELETE ON account_group_memberships_manual FOR EACH STATEMENT
EXECUTE FUNCTION refresh_derived_memberships_on_change ();

CREATE VIEW current_room_occupants AS
SELECT
  a.id,
  COALESCE(a.full_name, a.label) AS full_name,
  COALESCE(a.display_name, split_part(a.full_name, ' ', 1)) AS display_name,
  r.label AS room_name,
  r.id AS room_id
FROM
  accounts_people a
  LEFT JOIN res_leases l ON a.id = l.tenant_id
  LEFT JOIN res_rooms r ON l.room_id = r.id;

CREATE VIEW group_email_recipients AS
SELECT
  ap.email AS account_email,
  ag.email as group_localpart
FROM
  accounts_people ap
  JOIN account_group_memberships agm ON ap.id = agm.account_id
  JOIN account_groups ag ON ag.id = agm.group_id
WHERE
  ap.email IS NOT NULL
  AND ap.manually_suspended = false
  AND ap.membership_end >= CURRENT_DATE;

--WHERE
--l.begins <= CURRENT_DATE
--AND (l.ends IS NULL OR l.ends >= CURRENT_DATE);

---
--- Products
--- Yes, ENUMs aren't usually super suitable for this type of thing, but products aren't at all dynamic here.
---
CREATE TYPE product_type AS ENUM('printer_color', 'printer_bw', 'bookable');

CREATE TYPE ledger_entry_type AS ENUM(
  'transfer',
  'unlimited_enabled',
  'unlimited_disabled'
);

---
--- Credits
---
CREATE TYPE transaction_status AS ENUM('pending', 'done', 'refunded');

CREATE TABLE credit_ledger (
  id uuid PRIMARY KEY default gen_random_uuid(),
  from_account uuid REFERENCES accounts_people (id) ON DELETE CASCADE,
  to_account uuid REFERENCES accounts_people (id) ON DELETE CASCADE,
  credits int,
  status transaction_status NOT NULL DEFAULT 'done',
  product product_type,
  created_at TIMESTAMPTZ DEFAULT NOW() NOT NULL,
  last_updated TIMESTAMPTZ,
  label text,
  refund_of uuid REFERENCES credit_ledger (id),
  entry_type ledger_entry_type NOT NULL DEFAULT 'transfer',
  -- Materialized from the rows that point here via refund_of
  credits_refunded int NOT NULL DEFAULT 0,
  CONSTRAINT credit_ledger_refund_not_exceeded CHECK (
    credits_refunded >= 0
    AND (
      credits IS NULL
      OR credits_refunded <= credits
    )
  ),
  CHECK (
    (
      entry_type = 'transfer'
      AND credits IS NOT NULL
      AND credits >= 0
    )
    OR (entry_type != 'transfer')
  ),
  CHECK (
    NOT (
      from_account IS NULL
      AND to_account IS NULL
    )
  )
);

CREATE INDEX credit_ledger_created_at_idx ON credit_ledger (created_at);

CREATE INDEX credit_ledger_from_idx ON credit_ledger (from_account);

CREATE INDEX credit_ledger_to_idx ON credit_ledger (to_account);

CREATE INDEX credit_ledger_refund_of_idx ON credit_ledger (refund_of) WHERE refund_of IS NOT NULL;

-- Update credits column in accounts table on transaction.
CREATE OR REPLACE FUNCTION materialize_transaction () RETURNS TRIGGER AS $$
BEGIN
    IF NEW.entry_type = 'transfer' THEN
        UPDATE accounts_people SET credit_balance = credit_balance - NEW.credits WHERE id = NEW.from_account AND credit_balance IS NOT NULL;
        UPDATE accounts_people SET credit_balance = credit_balance + NEW.credits WHERE id = NEW.to_account AND credit_balance IS NOT NULL;
    ELSIF NEW.entry_type = 'unlimited_enabled' THEN
        UPDATE accounts_people SET credit_balance = NULL WHERE id = NEW.from_account;
    ELSIF NEW.entry_type = 'unlimited_disabled' THEN
        -- Restore pre-unlimited credit balance
        UPDATE accounts_people SET credit_balance = COALESCE((
            SELECT credits FROM credit_ledger
            WHERE from_account = NEW.to_account
              AND entry_type = 'unlimited_enabled'
            ORDER BY created_at DESC LIMIT 1
        ), 0) WHERE id = NEW.to_account;
    END IF;
    RETURN NEW;
END;
$$ LANGUAGE plpgsql;

CREATE OR REPLACE FUNCTION prevent_credit_ledger_update () RETURNS TRIGGER AS $$
BEGIN
    IF NEW.from_account IS DISTINCT FROM OLD.from_account
        OR NEW.to_account IS DISTINCT FROM OLD.to_account
        OR NEW.credits IS DISTINCT FROM OLD.credits
        OR NEW.product IS DISTINCT FROM OLD.product
        OR NEW.created_at IS DISTINCT FROM OLD.created_at
        OR NEW.label IS DISTINCT FROM OLD.label
        OR NEW.refund_of IS DISTINCT FROM OLD.refund_of
        OR NEW.entry_type IS DISTINCT FROM OLD.entry_type
    THEN
        RAISE EXCEPTION 'Only the "status" column may be changed in the credit ledger';
    END IF;
    NEW.last_updated = NOW();
    RETURN NEW;
END;
$$ LANGUAGE plpgsql;

-- Accumulate the refunded amount on the target
CREATE OR REPLACE FUNCTION set_refund_status () RETURNS TRIGGER AS $$
BEGIN
    IF NEW.refund_of IS NOT NULL THEN
        UPDATE credit_ledger SET
            credits_refunded = credits_refunded + COALESCE(NEW.credits, 0),
            status = CASE
                WHEN credits_refunded + COALESCE(NEW.credits, 0) >= credits THEN 'refunded'
                ELSE status
            END
        WHERE id = NEW.refund_of;
    END IF;

    RETURN NEW;
END;
$$ LANGUAGE plpgsql;

-- Create the triggers
CREATE TRIGGER tg_materialize_transaction
AFTER INSERT ON credit_ledger FOR EACH ROW
EXECUTE FUNCTION materialize_transaction ();

CREATE TRIGGER tg_prevent_credit_ledger_update
BEFORE UPDATE ON credit_ledger FOR EACH ROW
EXECUTE FUNCTION prevent_credit_ledger_update ();

CREATE TRIGGER tg_set_refund_status
AFTER INSERT ON credit_ledger FOR EACH ROW
EXECUTE FUNCTION set_refund_status ();

-- Prevent modifications to the audit log
-- CREATE OR REPLACE FUNCTION prevent_updates()
-- RETURNS TRIGGER AS $$
-- BEGIN
--     RAISE EXCEPTION 'Modifications are not allowed for this table';
-- END;
-- $$ LANGUAGE plpgsql;
