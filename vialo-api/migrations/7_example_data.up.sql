-- Add up migration script here
create or replace procedure ddl_create_example_rooms () language plpgsql as $$
declare
  _sql varchar;
  ii text;
  jj text;
begin
    FOR i IN 1..15 LOOP
        ii := LPAD(i::text, 2, '0');
        FOR j IN 1..15 LOOP
        jj := LPAD(j::text, 2, '0');
        _sql := CONCAT(
            'insert into res_rooms (label, floor, capacity) VALUES (''',CONCAT(ii,jj), ''',
            ''',i,''', 1);'
        );
        execute _sql;
        END LOOP;
    END LOOP;
end;
$$;

call ddl_create_example_rooms ();

-- Add up migration script here
create or replace procedure ddl_create_example_realms () language plpgsql as $$
declare
  _sql varchar;
begin
    FOR i IN 1..255 LOOP
        _sql := CONCAT(
            'insert into net_realms (ipv4_subnet, ipv4_nat, ipv6_prefix, ipv4_dns, ipv4_router, vlan) VALUES (''10.123.',
            i, '.0/24'',
            ''189.213.123.', i, ''', null, ''10.123.',
            i, '.2/24'', ''10.123.',
            i, '.1/24'', 111);'
        );
        execute _sql;
    END LOOP;
end;
$$;

call ddl_create_example_realms ();
