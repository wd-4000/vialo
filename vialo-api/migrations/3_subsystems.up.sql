---
--- Subsystems
---
CREATE TYPE subsystem_type AS ENUM(
  'app',
  'bookable',
  'printer',
  'ppsk',
  'email',
  'sysop'
);

CREATE TYPE job_status AS ENUM('pending', 'processing', 'done', 'error');

CREATE TABLE subsystem_printer_context (
  id uuid UNIQUE REFERENCES accounts_people (id) ON DELETE SET NULL,
  printer_id int PRIMARY KEY,
  printer_username text,
  printer_password text,
  bw int NOT NULL DEFAULT 0,
  color int NOT NULL DEFAULT 0
);

CREATE TABLE subsystem_printer_pricing (
  begins TIMESTAMPTZ NOT NULL DEFAULT NOW(),
  product product_type NOT NULL,
  unit_price int NOT NULL CHECK (unit_price > 0),
  PRIMARY KEY (begins, product)
);

-- TODO Add trigger to remove printer user account
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
           created_at,
           last_updated
       )
       VALUES (
           r_account_id,
           NULL,
           r_product,
           p_sum,
           'pending',
           p_event_date,
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
