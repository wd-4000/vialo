---
--- Table Log
--- Generic log used to track changes in tables.
---
CREATE TYPE tgop AS ENUM('INSERT', 'UPDATE', 'DELETE');

CREATE TABLE IF NOT EXISTS table_log (
  id int PRIMARY KEY GENERATED ALWAYS AS IDENTITY,
  table_name TEXT NOT NULL,
  record_id int, -- NB/TODO: Schedule PII-related logs for deletion when a user gets deleted!
  record_uuid uuid,
  record_text text, -- For enum/text/timestamp PKs
  operation_type tgop NOT NULL,
  created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
  delete_on TIMESTAMPTZ,
  caused_by_account uuid REFERENCES accounts_people (id) ON DELETE SET NULL,
  caused_by_subsystem subsystem_type,
  diff jsonb,
  transaction_id bigint
);

-- attribution
CREATE OR REPLACE FUNCTION validate_insert_trigger () RETURNS TRIGGER AS $$
BEGIN
    IF NOT (
        (NEW.caused_by_account IS NULL AND NEW.caused_by_subsystem IS NOT NULL) OR
        (NEW.caused_by_account IS NOT NULL AND NEW.caused_by_subsystem IS NULL)
    ) THEN
        RAISE EXCEPTION 'Unattributed or overattributed audit log entry: account: %, subsystem: %',
            COALESCE(NEW.caused_by_account::text, 'NULL'),
            COALESCE(NEW.caused_by_subsystem::text, 'NULL');
    END IF;
    RETURN NEW;
END;
$$ LANGUAGE plpgsql;

CREATE TRIGGER enforce_check_on_insert
after insert ON table_log FOR EACH ROW
EXECUTE FUNCTION validate_insert_trigger ();

-- immutability
CREATE OR REPLACE FUNCTION prevent_table_log_update () RETURNS TRIGGER AS $$
BEGIN
    RAISE EXCEPTION 'Modifications to the audit log are not allowed';
END;
$$ LANGUAGE plpgsql;

CREATE TRIGGER enforce_table_log_immutability BEFORE
UPDATE ON table_log FOR EACH ROW
EXECUTE FUNCTION prevent_table_log_update ();

--
CREATE INDEX table_name_and_record_id_idx ON table_log (table_name, record_id);

CREATE INDEX table_name_and_record_uuid_idx ON table_log (table_name, record_uuid);

CREATE INDEX created_at_idx ON table_log (created_at);

-- reading
CREATE OR REPLACE FUNCTION resolve_i18n_in_diff (p_diff jsonb, p_i18n_cols text[]) RETURNS jsonb AS $$
DECLARE
    v_col_name text;
    v_resolved_diff jsonb;
    v_col_val jsonb;
    v_from_id int;
    v_to_id int;
BEGIN
    IF p_diff IS NULL OR p_diff = '{}'::jsonb THEN
        RETURN p_diff;
    END IF;

    v_resolved_diff := p_diff;

    FOREACH v_col_name IN ARRAY p_i18n_cols
    LOOP
        IF v_resolved_diff ? v_col_name THEN
            v_col_val := v_resolved_diff -> v_col_name;

            -- If it's an UPDATE diff with 'from'/'to' keys
            IF jsonb_typeof(v_col_val) = 'object' AND (v_col_val ? 'from' OR v_col_val ? 'to') THEN
                v_from_id := (v_col_val ->> 'from')::int;
                v_to_id := (v_col_val ->> 'to')::int;

                v_resolved_diff := jsonb_set(
                    v_resolved_diff,
                    array[v_col_name],
                    jsonb_build_object(
                        'from', CASE WHEN v_from_id IS NOT NULL THEN get_i18n_all_string_translations(v_from_id) ELSE NULL END,
                        'to',   CASE WHEN v_to_id IS NOT NULL THEN get_i18n_all_string_translations(v_to_id) ELSE NULL END
                    )
                );

            -- If it's an INSERT/DELETE diff with just a raw integer ID
            ELSIF jsonb_typeof(v_col_val) = 'number' THEN
                v_resolved_diff := jsonb_set(
                    v_resolved_diff,
                    array[v_col_name],
                    COALESCE(get_i18n_all_string_translations((v_col_val)::text::int), 'null'::jsonb)
                );
            END IF;
        END IF;
    END LOOP;

    RETURN v_resolved_diff;
END;
$$ LANGUAGE plpgsql STABLE;

CREATE OR REPLACE FUNCTION table_log_trigger () RETURNS TRIGGER AS $$
DECLARE
    p_new_data jsonb;
    p_old_data jsonb;
    p_key text;
    p_diff jsonb;
    p_account_id uuid;
    p_subsystem subsystem_type;
    p_tx_id bigint;
BEGIN
    SELECT INTO p_account_id CAST(nullif(current_setting('app.account_id', true), '') AS uuid);
    SELECT INTO p_subsystem subsystem_type(nullif(current_setting('app.subsystem', true), ''));

    IF p_account_id IS NULL AND p_subsystem IS NULL THEN
        RAISE EXCEPTION 'Changes without attribution to table % are not allowed', TG_TABLE_NAME;
    END IF;

    p_diff := '{}';
    p_tx_id := pg_current_xact_id_if_assigned()::text::bigint;

    IF TG_OP = 'INSERT' THEN
        p_diff := to_jsonb(NEW);

    ELSIF TG_OP = 'UPDATE' THEN
        p_new_data := to_jsonb(NEW);
        p_old_data := to_jsonb(OLD);

        FOR p_key IN SELECT jsonb_object_keys(p_new_data) INTERSECT SELECT jsonb_object_keys(p_old_data)
        LOOP
            IF (p_new_data ->> p_key) IS DISTINCT FROM (p_old_data ->> p_key) THEN
                p_diff := p_diff || jsonb_build_object(
                    p_key, jsonb_build_object(
                        'from', p_old_data -> p_key,
                        'to',   p_new_data -> p_key
                    )
                );
            END IF;
        END LOOP;

        -- Skip logging empty UPDATEs
        IF p_diff = '{}'::jsonb THEN
            RETURN NULL;
        END IF;

    ELSIF TG_OP = 'DELETE' THEN
        p_diff := to_jsonb(OLD);

    END IF;

    IF TG_ARGV[0] = 'integer' THEN
        INSERT INTO table_log (table_name, record_id, operation_type, caused_by_account, caused_by_subsystem, diff, transaction_id)
        VALUES (TG_TABLE_NAME, COALESCE(p_diff ->> TG_ARGV[1], p_new_data->> TG_ARGV[1])::int, CAST(TG_OP as tgop), p_account_id, p_subsystem, p_diff, p_tx_id);
    ELSIF TG_ARGV[0] = 'text' THEN
        INSERT INTO table_log (table_name, record_text, operation_type, caused_by_account, caused_by_subsystem, diff, transaction_id)
        VALUES (TG_TABLE_NAME, COALESCE(p_diff ->> TG_ARGV[1], p_new_data->> TG_ARGV[1])::text, CAST(TG_OP as tgop), p_account_id, p_subsystem, p_diff, p_tx_id);
    ELSE
        INSERT INTO table_log (table_name, record_uuid, operation_type, caused_by_account, caused_by_subsystem, diff, transaction_id)
        VALUES (TG_TABLE_NAME, COALESCE(p_diff ->> TG_ARGV[1], p_new_data->> TG_ARGV[1])::uuid, CAST(TG_OP as tgop), p_account_id, p_subsystem, p_diff, p_tx_id);
    END IF;

    RETURN NULL;
END;
$$ LANGUAGE plpgsql;

-- Special triggers
create trigger tg_table_log
after insert
or
update
or delete on account_group_app_roles for each row
execute FUNCTION table_log_trigger ('uuid', 'group_id');

create trigger tg_table_log
after insert
or
update
or delete on res_rooms for each row
execute FUNCTION table_log_trigger ('uuid', 'id');

create trigger tg_table_log
after insert
or
update
or delete on account_group_memberships for each row
execute FUNCTION table_log_trigger ('uuid', 'group_id');

create trigger tg_table_log
after insert
or
update
or delete on bookable_schema_assignments for each row
execute FUNCTION table_log_trigger ('integer', 'asset_id');

create trigger tg_table_log
after insert
or
update
or delete on board_group_perms for each row
execute FUNCTION table_log_trigger ('integer', 'board_id');

create trigger tg_table_log
after insert
or
update
or delete on board_pinned_posts for each row
execute FUNCTION table_log_trigger ('integer', 'board_id');

create trigger tg_table_log
after insert
or
update
or delete on boards for each row
execute FUNCTION table_log_trigger ('integer', 'id');

create trigger tg_table_log
after insert
or
update
or delete on subsystem_printer_pricing for each row
execute FUNCTION table_log_trigger ('text', 'product');

-- Create audit table trigger for tables where such a trigger doesn't exist
create or replace procedure ddl_create_audit_trigger_on_all_tables () language plpgsql as $$
declare
  _sql varchar;
begin
  for _sql in select concat (
      'create trigger tg_table_log after insert or update or delete on ',
      quote_ident(t.table_name),
      $s$ for each row execute FUNCTION table_log_trigger ('$s$, c.data_type, $s$','$s$, c.column_name, $s$'); $s$
    )
    from
      information_schema.tables t
      JOIN information_schema.table_constraints tc ON tc.table_name = t.table_name
      JOIN information_schema.constraint_column_usage AS ccu USING (constraint_schema, constraint_name)
      JOIN information_schema.columns AS c ON c.table_schema = tc.constraint_schema
      AND tc.table_name = c.table_name AND ccu.column_name = c.column_name AND tc.constraint_type = 'PRIMARY KEY'

    where
      t.table_type = 'BASE TABLE' and
      t.table_schema not in ('pg_catalog', 'information_schema') and
      t.table_schema not like 'pg_toast%' and
      t.table_name not like '_sqlx%' and
      t.table_name != 'table_log'


  loop
    BEGIN
        execute _sql;
        EXCEPTION
        WHEN duplicate_object THEN
            NULL;
    END;
  end loop;
end;
$$;

-- create after update trigger on all tables
call ddl_create_audit_trigger_on_all_tables ();
