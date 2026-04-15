-- Accounts, Groups and Credits
CREATE EXTENSION citext;

--
-- Accounts
--
-- Table that stores Ory Kratos identity info
CREATE TABLE identities (id uuid PRIMARY KEY, full_name text, email text);

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
  full_name text,
  manually_suspended boolean not null default false, -- True = No access to any syiervices.
  credit_balance int NOT NULL DEFAULT 0 -- Automatically updated, enabled by default!!
);

CREATE INDEX account_auth_idx ON accounts_people (auth_id);

-- Account groups
CREATE TABLE account_groups (
  id uuid PRIMARY KEY REFERENCES accounts (id),
  label text NOT NULL, -- Group name
  icon text,
  email citext UNIQUE,
  public bool NOT NULL
);

CREATE TYPE group_role AS ENUM('member', 'manager');

CREATE TABLE account_group_memberships (
  account_id uuid NOT NULL REFERENCES accounts_people (id) ON DELETE CASCADE,
  group_id uuid NOT NULL REFERENCES account_groups (id),
  role group_role NOT NULL,
  PRIMARY KEY (account_id, group_id)
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

create trigger tg_account_class before insert
or
update
or delete on accounts_people for each row
execute FUNCTION enforce_account_class ();

create trigger tg_account_class before insert
or
update
or delete on account_groups for each row
execute FUNCTION enforce_account_class ();

CREATE TYPE app_role AS ENUM(
  'account_manager',
  'credit_manager',
  'bookable_manager',
  'board_manager',
  'home_manager',
  'network_manager',
  'history_viewer',
  'health_viewer'
);

CREATE TABLE account_group_app_roles (
  role app_role,
  group_id uuid REFERENCES account_groups (id),
  PRIMARY KEY (group_id, role)
);

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

CREATE VIEW current_room_occupants AS
SELECT
  a.id,
  COALESCE(a.full_name, a.label) AS full_name,
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

CREATE OR REPLACE FUNCTION account_role_exists (acid uuid, rl app_role) RETURNS BOOLEAN AS $$
BEGIN
    RETURN (
        SELECT COUNT(*) > 0
        FROM account_group_memberships agm
        JOIN account_group_app_roles agra
        ON agm.group_id = agra.group_id
        WHERE agm.account_id = acid AND agra.role = rl
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

---
--- Products
--- Yes, ENUMs aren't usually super suitable for this type of thing, but products aren't at all dynamic here.
---
CREATE TYPE product_type AS ENUM('printer_color', 'printer_bw', 'bookable');

---
--- Credits
---
CREATE TYPE transaction_status AS ENUM('pending', 'done', 'refunded');

CREATE TABLE credit_ledger (
  id uuid PRIMARY KEY default gen_random_uuid(),
  from_account uuid REFERENCES accounts_people (id) ON DELETE CASCADE,
  to_account uuid REFERENCES accounts_people (id) ON DELETE CASCADE,
  credits int NOT NULL,
  status transaction_status NOT NULL DEFAULT 'done',
  product product_type,
  created_at TIMESTAMPTZ DEFAULT NOW() NOT NULL,
  label text,
  last_updated TIMESTAMPTZ,
  CHECK (credits > 0),
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

-- Update credits column in accounts table on transaction.
CREATE OR REPLACE FUNCTION materialize_transaction () RETURNS TRIGGER AS $$
BEGIN
    IF TG_OP = 'INSERT' AND NEW.status <> 'refunded' THEN
            UPDATE accounts_people SET credit_balance = credit_balance - NEW.credits WHERE id = NEW.from_account;
            UPDATE accounts_people SET credit_balance = credit_balance + NEW.credits WHERE id = NEW.to_account;
    ELSIF TG_OP = 'UPDATE' THEN
        -- Make sure no funny business is going on with the update operation.
        IF (NEW.from_account IS DISTINCT FROM OLD.from_account
            OR NEW.to_account IS DISTINCT FROM OLD.to_account
            OR NEW.credits IS DISTINCT FROM OLD.credits
            OR NEW.created_at IS DISTINCT FROM OLD.created_at) THEN
            RAISE EXCEPTION 'Updates to columns other than "status" and "last_updated" are not allowed';
        END IF;

        -- Reverse the effects of the old transaction if it was not refunded already.
        IF OLD.status <> 'refunded' THEN
            UPDATE accounts_people SET credit_balance = credit_balance + OLD.credits WHERE id = OLD.from_account;
            UPDATE accounts_people SET credit_balance = credit_balance - OLD.credits WHERE id = OLD.to_account;
        END IF;

        -- Apply the new transaction effects if it isn't refunded now.
        IF NEW.status <> 'refunded' THEN
            UPDATE accounts_people SET credit_balance = credit_balance - NEW.credits WHERE id = NEW.from_account;
            UPDATE accounts_people SET credit_balance = credit_balance + NEW.credits WHERE id = NEW.to_account;
        END IF;
    END IF;

    RETURN NEW;
END;
$$ LANGUAGE plpgsql;

-- Create the trigger
CREATE TRIGGER tg_materialize_transaction
AFTER INSERT
OR
UPDATE ON credit_ledger FOR EACH ROW
EXECUTE FUNCTION materialize_transaction ();

-- Prevent modifications to the audit log
-- CREATE OR REPLACE FUNCTION prevent_updates()
-- RETURNS TRIGGER AS $$
-- BEGIN
--     RAISE EXCEPTION 'Modifications are not allowed for this table';
-- END;
-- $$ LANGUAGE plpgsql;
