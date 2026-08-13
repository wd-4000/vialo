---
--- Subsystems
---
CREATE TYPE subsystem_type AS ENUM(
  'app',
  'bookable',
  'printer',
  'ppsk',
  'email',
  'sysop',
  'dhcp'
);

CREATE TYPE job_status AS ENUM('pending', 'processing', 'done', 'error');

CREATE TABLE subsystem_printer_context (
  id uuid UNIQUE REFERENCES accounts_people (id) ON DELETE SET NULL,
  printer_id int PRIMARY KEY,
  printer_username text,
  bw int NOT NULL DEFAULT 0,
  color int NOT NULL DEFAULT 0
);

CREATE TABLE subsystem_printer_pricing (
  begins TIMESTAMPTZ NOT NULL DEFAULT NOW(),
  product product_type NOT NULL,
  unit_price int NOT NULL CHECK (unit_price > 0),
  PRIMARY KEY (begins, product)
);

-- Create the trigger function
--
CREATE OR REPLACE FUNCTION log_printer_transaction (
  r_account_id uuid,
  r_product product_type,
  r_amount_purchased INT
) RETURNS void AS $$
DECLARE
    p_event_date timestamptz;
    p_sum int;
    p_ledger_id uuid;
    p_unit_price int;
BEGIN
        p_event_date := NOW();

        SELECT unit_price INTO p_unit_price FROM subsystem_printer_pricing
        WHERE product = r_product
          AND begins <= p_event_date
        ORDER BY begins DESC
        LIMIT 1;

        IF p_unit_price IS NULL THEN
            RAISE EXCEPTION 'adm/no_price_configured' USING DETAIL = format('product: %s', r_product);
        END IF;

        p_sum := r_amount_purchased * p_unit_price;


        INSERT INTO credit_ledger (
           from_account,
           to_account,
           product,
           credits,
           status,
           created_at
       )
       VALUES (
           r_account_id,
           NULL,
           r_product,
           p_sum,
           'pending',
           p_event_date
       ) RETURNING id INTO p_ledger_id;
END;
$$ LANGUAGE plpgsql;

CREATE OR REPLACE FUNCTION log_printer_transaction_color () RETURNS TRIGGER AS $$
DECLARE
    column_difference int;
BEGIN
    -- Dynamically calculate the difference based on the column_name argument
   column_difference := NEW.color - OLD.color;

    -- Only insert a new transaction if there is a positive difference
    IF column_difference > 0 THEN
        PERFORM log_printer_transaction(NEW.id, 'printer_color'::product_type, column_difference);
    END IF;

    RETURN NEW;
END;
$$ LANGUAGE plpgsql;

CREATE OR REPLACE FUNCTION log_printer_transaction_bw () RETURNS TRIGGER AS $$
DECLARE
    column_difference int;
BEGIN

    -- Dynamically calculate the difference based on the column_name argument
   column_difference := NEW.bw - OLD.bw;

    -- Only insert a new transaction if there is a positive difference
    IF column_difference > 0 THEN
        PERFORM log_printer_transaction(NEW.id, 'printer_bw'::product_type, column_difference);
    END IF;

    RETURN NEW;
END;
$$ LANGUAGE plpgsql;

-- Attach the triggers to the 'subsystem_printer_context' table
CREATE TRIGGER trigger_bw_update
AFTER
UPDATE OF bw ON subsystem_printer_context FOR EACH ROW
EXECUTE FUNCTION log_printer_transaction_bw ();

CREATE TRIGGER trigger_color_update
AFTER
UPDATE OF color ON subsystem_printer_context FOR EACH ROW
EXECUTE FUNCTION log_printer_transaction_color ();

CREATE TABLE subsystem_jobs (
  id int PRIMARY KEY GENERATED ALWAYS AS IDENTITY,
  subsystem subsystem_type NOT NULL,
  data jsonb NOT NULL,
  created_at timestamptz NOT NULL DEFAULT NOW(),
  last_updated timestamptz,
  status job_status NOT NULL
);

CREATE INDEX idx_subsystem_jobs_account_id ON subsystem_jobs (subsystem, (data ->> 'account_id'))
WHERE
  status != 'done';

-- The amenities login lives on the person, the printer mirrors it on the device.
CREATE OR REPLACE FUNCTION amenities_login_printer_sync () RETURNS TRIGGER AS $$
BEGIN
    -- Nothing to push until both halves exist
    IF NEW.amenities_username IS NULL OR NEW.amenities_pin IS NULL THEN
        RETURN NULL;
    END IF;

    -- UPDATE OF fires on assignment, not on change
    IF TG_OP = 'UPDATE'
       AND NEW.amenities_username IS NOT DISTINCT FROM OLD.amenities_username
       AND NEW.amenities_pin IS NOT DISTINCT FROM OLD.amenities_pin THEN
        RETURN NULL;
    END IF;

    INSERT INTO subsystem_jobs (subsystem, data, created_at, last_updated, status)
    VALUES (
      'printer',
      jsonb_build_object('type', 'sync_account', 'account_id', NEW.id),
      NOW(),
      NOW(),
      'pending'
    );

    RETURN NULL;
END;
$$ LANGUAGE plpgsql;

CREATE TRIGGER tg_amenities_login_printer_sync
AFTER INSERT
OR
UPDATE OF amenities_username,
amenities_pin ON accounts_people FOR EACH ROW
EXECUTE FUNCTION amenities_login_printer_sync ();

-- Deleting a person-linked mirror row also deletes the device account.
CREATE OR REPLACE FUNCTION printer_context_delete_sync () RETURNS TRIGGER AS $$
BEGIN
    IF OLD.id IS NULL THEN
        RETURN NULL;
    END IF;

    INSERT INTO subsystem_jobs (subsystem, data, created_at, last_updated, status)
    VALUES (
      'printer',
      jsonb_build_object('type', 'delete_account', 'printer_id', OLD.printer_id),
      NOW(),
      NOW(),
      'pending'
    );

    RETURN NULL;
END;
$$ LANGUAGE plpgsql;

CREATE TRIGGER tg_printer_context_delete_sync
AFTER DELETE ON subsystem_printer_context FOR EACH ROW
EXECUTE FUNCTION printer_context_delete_sync ();

-- Credit changes need a limit update (limits are counter + balance/price).
-- Printer billing rows don't, balance and counter move together.
CREATE OR REPLACE FUNCTION enqueue_account_limit_update () RETURNS TRIGGER AS $$
DECLARE
    p_account_id uuid;
BEGIN
    IF NOT (
        (NEW.entry_type = 'transfer'
         AND (NEW.product IS NULL OR NEW.product NOT IN ('printer_bw', 'printer_color')))
        OR NEW.entry_type IN ('unlimited_enabled', 'unlimited_disabled')
    ) THEN
        RETURN NULL;
    END IF;

    -- Both sides of a transfer may change balance
    FOR p_account_id IN
        SELECT unnest(ARRAY[NEW.from_account, NEW.to_account])
    LOOP
        CONTINUE WHEN p_account_id IS NULL;

        -- No printer access, nothing to limit
        CONTINUE WHEN NOT EXISTS (
            SELECT 1 FROM subsystem_printer_context WHERE id = p_account_id
        );

        INSERT INTO subsystem_jobs (subsystem, data, created_at, last_updated, status)
        VALUES (
          'printer',
          jsonb_build_object('type', 'update_account_limit', 'account_id', p_account_id),
          NOW(),
          NOW(),
          'pending'
        );
    END LOOP;

    RETURN NULL;
END;
$$ LANGUAGE plpgsql;

CREATE TRIGGER tg_enqueue_account_limit_update
AFTER INSERT ON credit_ledger FOR EACH ROW
EXECUTE FUNCTION enqueue_account_limit_update ();

-- Dedup identical pending jobs.
CREATE OR REPLACE FUNCTION dedup_subsystem_jobs () RETURNS TRIGGER AS $$
BEGIN
    IF EXISTS (
        SELECT 1 FROM subsystem_jobs
        WHERE subsystem = NEW.subsystem
          AND status = 'pending'
          AND data = NEW.data
    ) THEN
        RETURN NULL;
    END IF;
    RETURN NEW;
END;
$$ LANGUAGE plpgsql;

CREATE TRIGGER tg_dedup_subsystem_jobs
BEFORE INSERT ON subsystem_jobs FOR EACH ROW
EXECUTE FUNCTION dedup_subsystem_jobs ();
