#!/bin/bash
set -e

admin_username=$(</run/secrets/postgres_root_username)
admin_password=$(</run/secrets/postgres_root_password)

username=$(</run/secrets/postgres_db_username)
password=$(</run/secrets/postgres_db_password)

PGPASSWORD=$admin_password psql -v ON_ERROR_STOP=1 --username "$admin_username" --dbname "$POSTGRES_DB" <<-EOSQL
	CREATE USER $username WITH ENCRYPTED PASSWORD '$password';
    GRANT pg_read_all_data TO $username;
    GRANT pg_write_all_data TO $username;
	GRANT CONNECT ON DATABASE $POSTGRES_DB TO $username;
EOSQL

# PGPASSWORD=$admin_password psql -v ON_ERROR_STOP=1 --username "$admin_username" --dbname "$POSTGRES_DB" < tables.sql