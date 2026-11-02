---
--- Subsystems
---
CREATE TYPE subsystem_type AS ENUM(
  'app',
  'bookable',
  'printer',
  'nms',
  'email',
  'sysop',
  'dhcp',
  'import',
  'media'
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

-- Latest configured price, or bust
CREATE OR REPLACE FUNCTION printer_unit_price (
  p_product product_type,
  p_at timestamptz
) RETURNS int AS $$
DECLARE
    p_unit_price int;
BEGIN
    SELECT unit_price INTO p_unit_price FROM subsystem_printer_pricing
    WHERE product = p_product
      AND begins <= p_at
    ORDER BY begins DESC
    LIMIT 1;

    IF p_unit_price IS NULL THEN
        RAISE EXCEPTION 'adm/no_price_configured' USING DETAIL = format('product: %s', p_product);
    END IF;

    RETURN p_unit_price;
END;
$$ LANGUAGE plpgsql STABLE;

-- Counters are identified by printer_id, so a delta only means anything within
-- one. Statement level keeps a full counter refresh to a single pass and only
-- prices the rows that actually advanced.
CREATE OR REPLACE FUNCTION log_printer_transactions () RETURNS TRIGGER AS $$
DECLARE
    p_event_date timestamptz := NOW();
BEGIN
    INSERT INTO credit_ledger (from_account, to_account, product, credits, status, created_at)
    SELECT n.id,
           NULL,
           d.product,
           d.amount * printer_unit_price (d.product, p_event_date),
           'pending'::transaction_status,
           p_event_date
    FROM new_rows n
      JOIN old_rows o ON o.printer_id = n.printer_id
      CROSS JOIN LATERAL (VALUES
        ('printer_bw'::product_type, n.bw - o.bw),
        ('printer_color'::product_type, n.color - o.color)
      ) AS d(product, amount)
    -- Unclaimed mirror rows have nobody to bill
    WHERE n.id IS NOT NULL
      AND d.amount > 0;

    RETURN NULL;
END;
$$ LANGUAGE plpgsql;

CREATE TRIGGER tg_log_printer_transactions
AFTER
UPDATE ON subsystem_printer_context
REFERENCING OLD TABLE AS old_rows NEW TABLE AS new_rows FOR EACH STATEMENT
EXECUTE FUNCTION log_printer_transactions ();

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
    IF TG_OP = 'INSERT' THEN
        INSERT INTO subsystem_jobs (subsystem, data, created_at, last_updated, status)
        SELECT 'printer'::subsystem_type,
               jsonb_build_object('type', 'sync_account', 'account_id', n.id),
               NOW(), NOW(), 'pending'::job_status
        FROM new_rows n
        WHERE n.amenities_username IS NOT NULL
          AND n.amenities_pin IS NOT NULL;
    ELSE
        -- Fires on any update, so check the halves actually moved
        INSERT INTO subsystem_jobs (subsystem, data, created_at, last_updated, status)
        SELECT 'printer'::subsystem_type,
               jsonb_build_object('type', 'sync_account', 'account_id', n.id),
               NOW(), NOW(), 'pending'::job_status
        FROM new_rows n
          JOIN old_rows o ON o.id = n.id
        WHERE n.amenities_username IS NOT NULL
          AND n.amenities_pin IS NOT NULL
          AND (n.amenities_username IS DISTINCT FROM o.amenities_username
               OR n.amenities_pin IS DISTINCT FROM o.amenities_pin);
    END IF;

    RETURN NULL;
END;
$$ LANGUAGE plpgsql;

CREATE TRIGGER tg_amenities_login_printer_insert_sync
AFTER INSERT ON accounts_people
REFERENCING NEW TABLE AS new_rows FOR EACH STATEMENT
EXECUTE FUNCTION amenities_login_printer_sync ();

CREATE TRIGGER tg_amenities_login_printer_update_sync
AFTER
UPDATE ON accounts_people
REFERENCING OLD TABLE AS old_rows NEW TABLE AS new_rows FOR EACH STATEMENT
EXECUTE FUNCTION amenities_login_printer_sync ();

-- Deleting a person-linked mirror row also deletes the device account.
CREATE OR REPLACE FUNCTION printer_context_delete_sync () RETURNS TRIGGER AS $$
BEGIN
    INSERT INTO subsystem_jobs (subsystem, data, created_at, last_updated, status)
    SELECT 'printer'::subsystem_type,
           jsonb_build_object('type', 'delete_account', 'printer_id', o.printer_id),
           NOW(), NOW(), 'pending'::job_status
    FROM old_rows o
    WHERE o.id IS NOT NULL;

    RETURN NULL;
END;
$$ LANGUAGE plpgsql;

CREATE TRIGGER tg_printer_context_delete_sync
AFTER DELETE ON subsystem_printer_context
REFERENCING OLD TABLE AS old_rows FOR EACH STATEMENT
EXECUTE FUNCTION printer_context_delete_sync ();

-- Credit changes need a limit update (limits are counter + balance/price).
-- Printer billing rows don't, balance and counter move together (unless it's a refund)
CREATE OR REPLACE FUNCTION enqueue_account_limit_update () RETURNS TRIGGER AS $$
BEGIN
    -- Both sides of a transfer may change balance
    INSERT INTO subsystem_jobs (subsystem, data, created_at, last_updated, status)
    SELECT DISTINCT
           'printer'::subsystem_type,
           jsonb_build_object('type', 'update_account_limit', 'account_id', a.account_id),
           NOW(), NOW(), 'pending'::job_status
    FROM new_rows n
      CROSS JOIN LATERAL (VALUES (n.from_account), (n.to_account)) AS a(account_id)
    WHERE (
            (n.entry_type = 'transfer'
             AND (n.product IS NULL OR n.product NOT IN ('printer_bw', 'printer_color') OR n.refund_of IS NOT NULL))
            OR n.entry_type IN ('unlimited_enabled', 'unlimited_disabled')
          )
      AND a.account_id IS NOT NULL
      -- No printer access, nothing to limit
      AND EXISTS (
        SELECT 1 FROM subsystem_printer_context WHERE id = a.account_id
      );

    RETURN NULL;
END;
$$ LANGUAGE plpgsql;

CREATE TRIGGER tg_enqueue_account_limit_update
AFTER INSERT ON credit_ledger
REFERENCING NEW TABLE AS new_rows FOR EACH STATEMENT
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
