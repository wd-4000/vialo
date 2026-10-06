--
-- Network
--
CREATE TYPE net_auth AS ENUM('username_password', 'password');

-- Networks are Enterprise WiFi, Compat WiFi, Ethernet etc.
CREATE TABLE net_networks (
  id int PRIMARY KEY GENERATED ALWAYS AS IDENTITY,
  label text UNIQUE NOT NULL,
  icon text,
  wired boolean NOT NULL,
  multi_device boolean NOT NULL, -- Multiple devices can use the same credentials
  auto_add_on_auth boolean NOT NULL, -- Logged in devices automatically get added to net_devices and hence obtain an IP (=false for Ethernet and Compat WiFi, for now)
  auto_add_via_dhcp boolean NOT NULL, -- Logged in devices automatically get added to net_devices and hence obtain an IP (=false for Ethernet and Compat WiFi, for now)
  auth net_auth, -- Auth type (NULL permitted yeah)
  gen_username int, -- Preferences for username/password generation
  gen_password int,

  active boolean NOT NULL DEFAULT true, -- New devices/credentials may be added
  migrate_to int REFERENCES net_networks(id), -- Seamlessly push devices to the new network

  ssid text, -- WiFi SSID to use in mobileconfig (NULL = use label)
  tls_trust text, -- TLS trusted server name for EAP (NULL = use org domain)
  outer_identity text, -- EAP outer identity (NULL = anonymous@org.domain)
  CHECK (migrate_to IS NULL OR (active AND migrate_to != id))
);

CREATE TYPE net_nms_type AS ENUM('unifi', 'mikrotik');

CREATE TABLE net_nms_connectors (
  id int PRIMARY KEY GENERATED ALWAYS AS IDENTITY,
  type net_nms_type NOT NULL,
  hostname text NOT NULL,
  api_key BYTEA,
  site_id text,
  username BYTEA,
  password BYTEA,
  CONSTRAINT nms_auth_check CHECK (
    (api_key IS NULL OR (username IS NULL AND password IS NULL))
    AND (username IS NULL) = (password IS NULL)
  )
);

CREATE TABLE net_network_nms_link (
  network_id int REFERENCES net_networks NOT NULL,
  nms_connector_id int REFERENCES net_nms_connectors NOT NULL,
  nms_id text NOT NULL,
  PRIMARY KEY (network_id, nms_connector_id)
);

-- Credentials are Enterprise WiFi username/password pairings, Compat WiFi passwords
CREATE TABLE net_cred (
  id uuid PRIMARY KEY default gen_random_uuid(),
  username text,
  password BYTEA,
  account_id uuid NOT NULL REFERENCES accounts (id) ON DELETE CASCADE,
  network_id int NOT NULL REFERENCES net_networks (id) ON DELETE CASCADE,
  last_updated timestamptz,
  UNIQUE (id, network_id, account_id) -- redundant but needed for the FK in net_device_creds
);

CREATE INDEX net_cred_idx ON net_cred (network_id, username, password);

CREATE INDEX idx_net_cred_account ON net_cred (account_id); -- accounts cascade here

CREATE OR REPLACE FUNCTION enqueue_ppsk_refresh () RETURNS void AS $$
    INSERT INTO subsystem_jobs (subsystem, data, created_at, last_updated, status)
    VALUES ('nms', jsonb_build_object('type', 'refresh'), NOW(), NOW(), 'pending');
$$ LANGUAGE sql;

-- A cred only reaches the NMS if its network is PPSK and actually linked to one
CREATE OR REPLACE FUNCTION ppsk_network_linked (p_network_id int) RETURNS boolean AS $$
    SELECT EXISTS (
        SELECT 1 FROM net_networks n
          JOIN net_network_nms_link nnl ON nnl.network_id = n.id
        WHERE n.id = p_network_id
          AND n.auth = 'password'
    );
$$ LANGUAGE sql STABLE;

CREATE OR REPLACE FUNCTION ppsk_account_has_cred (p_account_id uuid) RETURNS boolean AS $$
    SELECT EXISTS (
        SELECT 1 FROM net_cred nc
        WHERE nc.account_id = p_account_id
          AND nc.password IS NOT NULL
          AND ppsk_network_linked (nc.network_id)
    );
$$ LANGUAGE sql STABLE;

-- Any cred change on a linked password network needs a refresh
CREATE OR REPLACE FUNCTION ppsk_cred_sync () RETURNS TRIGGER AS $$
DECLARE
    p_changed boolean;
BEGIN
    IF TG_OP = 'INSERT' THEN
        SELECT EXISTS (
            SELECT 1 FROM new_rows
            WHERE password IS NOT NULL AND ppsk_network_linked (network_id)
        ) INTO p_changed;
    ELSIF TG_OP = 'DELETE' THEN
        SELECT EXISTS (
            SELECT 1 FROM old_rows
            WHERE password IS NOT NULL AND ppsk_network_linked (network_id)
        ) INTO p_changed;
    ELSE
        -- Fires on any update, so diff what actually gets pushed
        -- account_id is in there because it picks the VLAN
        WITH o AS (
            SELECT account_id, password, network_id FROM old_rows
            WHERE password IS NOT NULL AND ppsk_network_linked (network_id)
        ), n AS (
            SELECT account_id, password, network_id FROM new_rows
            WHERE password IS NOT NULL AND ppsk_network_linked (network_id)
        )
        SELECT EXISTS (TABLE o EXCEPT ALL TABLE n)
            OR EXISTS (TABLE n EXCEPT ALL TABLE o) INTO p_changed;
    END IF;

    IF p_changed THEN
        PERFORM enqueue_ppsk_refresh ();
    END IF;

    RETURN NULL;
END;
$$ LANGUAGE plpgsql;

-- column filter
CREATE TRIGGER tg_ppsk_cred_insert_sync
AFTER INSERT ON net_cred
REFERENCING NEW TABLE AS new_rows FOR EACH STATEMENT
EXECUTE FUNCTION ppsk_cred_sync ();

CREATE TRIGGER tg_ppsk_cred_delete_sync
AFTER DELETE ON net_cred
REFERENCING OLD TABLE AS old_rows FOR EACH STATEMENT
EXECUTE FUNCTION ppsk_cred_sync ();

CREATE TRIGGER tg_ppsk_cred_update_sync
AFTER
UPDATE ON net_cred
REFERENCING OLD TABLE AS old_rows NEW TABLE AS new_rows FOR EACH STATEMENT
EXECUTE FUNCTION ppsk_cred_sync ();

-- Realms are collections of subnet, nat and ipv6 prefix.
CREATE TABLE net_realms (
  id uuid PRIMARY KEY default gen_random_uuid(),
  ipv4_subnet cidr,
  ipv4_nat inet,
  ipv4_router inet,
  ipv4_dns inet,
  ipv4_dhcp_server inet, -- vialo-dhcp's own address on this subnet. Falls back to the configured siaddr
  ipv6_prefix cidr,
  vlan int,
  -- /20 = 4094 addresses; a mistyped prefix below that takes minutes to materialise
  CHECK (ipv4_subnet IS NULL OR masklen(ipv4_subnet) BETWEEN 20 AND 30),
  EXCLUDE USING gist (ipv4_subnet inet_ops WITH &&) -- No two realms may share an address
);

-- Realm validation + IP pool management. AFTER, so the CHECK constraints are evaluated
-- before we expand the subnet, and so the realm row already exists for the FK.
CREATE OR REPLACE FUNCTION sync_realm_ip () RETURNS TRIGGER AS $$
DECLARE
    pool inet[];
BEGIN
    IF NEW.ipv4_subnet IS NOT NULL THEN
        -- Realm-vs-realm overlap is handled by the exclusion constraint; this catches
        -- pool rows that somehow outlived their realm's subnet.
        IF EXISTS (SELECT 1 FROM net_ip_assignments WHERE NEW.ipv4_subnet && ipv4_addr AND realm_id != NEW.id) THEN
            RAISE EXCEPTION 'overlap';
        END IF;

        pool := expand_cidr(NEW.ipv4_subnet, ARRAY[NEW.ipv4_router, NEW.ipv4_dns, NEW.ipv4_dhcp_server]);

        -- Retire entries that left the pool: subnet moved, or router/dns now excludes them
        DELETE FROM net_ip_assignments WHERE realm_id = NEW.id AND NOT (ipv4_addr = ANY (pool));

        -- Insert/update IP entries. Addresses still in the pool keep their occupant.
        INSERT INTO net_ip_assignments (ipv4_addr, realm_id) VALUES (unnest(pool), NEW.id) ON CONFLICT (ipv4_addr)
        DO UPDATE SET realm_id = NEW.id;
    ELSE
        DELETE FROM net_ip_assignments WHERE realm_id = NEW.id;
    END IF;

    RETURN NULL;
END;
$$ LANGUAGE plpgsql;

-- Realm deletion is handled by the net_ip_assignments FK cascade.
CREATE TRIGGER tg_sync_realm_ip AFTER INSERT OR UPDATE ON net_realms
  FOR EACH ROW EXECUTE FUNCTION sync_realm_ip ();

CREATE TABLE net_realm_assignments (
  account_id uuid NOT NULL REFERENCES accounts (id) ON DELETE CASCADE, -- TODO/KEEPTODO: Make sure this is handled gracefully and in a legally compliant manner.
  realm_id uuid NOT NULL REFERENCES net_realms (id) ON DELETE CASCADE,
  PRIMARY KEY (account_id)
);
-- reassigning an account can change VLAN without touching net_cred
CREATE OR REPLACE FUNCTION ppsk_realm_assignment_sync () RETURNS TRIGGER AS $$
DECLARE
    p_changed boolean;
BEGIN
    IF TG_OP = 'INSERT' THEN
        SELECT EXISTS (
            SELECT 1 FROM new_rows WHERE ppsk_account_has_cred (account_id)
        ) INTO p_changed;
    ELSIF TG_OP = 'DELETE' THEN
        SELECT EXISTS (
            SELECT 1 FROM old_rows WHERE ppsk_account_has_cred (account_id)
        ) INTO p_changed;
    ELSE
        WITH o AS (
            SELECT account_id, realm_id FROM old_rows
            WHERE ppsk_account_has_cred (account_id)
        ), n AS (
            SELECT account_id, realm_id FROM new_rows
            WHERE ppsk_account_has_cred (account_id)
        )
        SELECT EXISTS (TABLE o EXCEPT ALL TABLE n)
            OR EXISTS (TABLE n EXCEPT ALL TABLE o) INTO p_changed;
    END IF;

    IF p_changed THEN
        PERFORM enqueue_ppsk_refresh ();
    END IF;

    RETURN NULL;
END;
$$ LANGUAGE plpgsql;

CREATE TRIGGER tg_ppsk_realm_assignment_insert_sync
AFTER INSERT ON net_realm_assignments
REFERENCING NEW TABLE AS new_rows FOR EACH STATEMENT
EXECUTE FUNCTION ppsk_realm_assignment_sync ();

CREATE TRIGGER tg_ppsk_realm_assignment_delete_sync
AFTER DELETE ON net_realm_assignments
REFERENCING OLD TABLE AS old_rows FOR EACH STATEMENT
EXECUTE FUNCTION ppsk_realm_assignment_sync ();

CREATE TRIGGER tg_ppsk_realm_assignment_update_sync
AFTER
UPDATE ON net_realm_assignments
REFERENCING OLD TABLE AS old_rows NEW TABLE AS new_rows FOR EACH STATEMENT
EXECUTE FUNCTION ppsk_realm_assignment_sync ();

-- Retagging a realm affects every cred assigned to it
CREATE OR REPLACE FUNCTION ppsk_realm_vlan_sync () RETURNS TRIGGER AS $$
DECLARE
    p_changed boolean;
BEGIN
    WITH o AS (
        SELECT id, vlan FROM old_rows r
        WHERE EXISTS (
            SELECT 1 FROM net_realm_assignments nra
            WHERE nra.realm_id = r.id AND ppsk_account_has_cred (nra.account_id)
        )
    ), n AS (
        SELECT id, vlan FROM new_rows r
        WHERE EXISTS (
            SELECT 1 FROM net_realm_assignments nra
            WHERE nra.realm_id = r.id AND ppsk_account_has_cred (nra.account_id)
        )
    )
    SELECT EXISTS (TABLE o EXCEPT ALL TABLE n)
        OR EXISTS (TABLE n EXCEPT ALL TABLE o) INTO p_changed;

    IF p_changed THEN
        PERFORM enqueue_ppsk_refresh ();
    END IF;

    RETURN NULL;
END;
$$ LANGUAGE plpgsql;

CREATE TRIGGER tg_ppsk_realm_vlan_sync
AFTER
UPDATE ON net_realms
REFERENCING OLD TABLE AS old_rows NEW TABLE AS new_rows FOR EACH STATEMENT
EXECUTE FUNCTION ppsk_realm_vlan_sync ();

-- Network devices
CREATE TABLE net_devices (
  id uuid PRIMARY KEY default gen_random_uuid(),
  account_id uuid NOT NULL REFERENCES accounts (id) ON DELETE CASCADE, -- Owner; every cred on the device must match
  label text,
  hostname text,
  realm_id uuid REFERENCES net_realms (id) ON DELETE SET NULL, -- Override for the user realm_id
  mobileconfig boolean default false NOT NULL, -- Whether the user should be offered a .mobileconfig as the primary option for connecting
  last_updated timestamptz default NOW(),
  last_seen timestamptz,
  UNIQUE (id, account_id) -- redundant but needed for the FK in net_device_creds
);

CREATE INDEX idx_net_devices_realm ON net_devices (realm_id);
CREATE INDEX idx_net_devices_account ON net_devices (account_id); -- accounts cascade here

CREATE TABLE net_device_creds (
  device_id uuid,
  cred_id uuid NOT NULL,
  network_id int NOT NULL,
  account_id uuid NOT NULL, -- Pinned to both ends, so a device's creds can't span accounts
  mac macaddr,
  PRIMARY KEY (device_id, cred_id),
  FOREIGN KEY (device_id, account_id) REFERENCES net_devices (id, account_id) ON DELETE CASCADE,
  FOREIGN KEY (cred_id, network_id, account_id) REFERENCES net_cred (id, network_id, account_id) ON UPDATE CASCADE ON DELETE CASCADE,
  CONSTRAINT unique_mac_network UNIQUE (mac, network_id),
  -- A MAC belongs to exactly one device, but may repeat across that device's creds
  EXCLUDE USING gist (mac WITH =, device_id WITH <>)
);

-- Needed by net_cred FK cascade
CREATE INDEX idx_net_device_creds_cred ON net_device_creds (cred_id, network_id);

-- IP address assignments
CREATE TABLE net_ip_assignments (
  ipv4_addr inet PRIMARY KEY,
  realm_id uuid NOT NULL REFERENCES net_realms (id) ON DELETE CASCADE DEFERRABLE INITIALLY IMMEDIATE, -- Realm this IP resides in. Essentially the IP pool.
  device_id uuid, -- Device attachment that this IP is currently occupied by
  cred_id uuid,
  expires timestamptz, -- Lease expiration date and time. 'infinity' = static reservation
  FOREIGN KEY (device_id, cred_id) REFERENCES net_device_creds (device_id, cred_id) ON DELETE CASCADE,
  UNIQUE (device_id, cred_id)
);

-- Helps the realm cascade/rebuild and the free address scan (expires IS NULL OR expires <= NOW())
CREATE INDEX idx_net_ip_assignments_realm ON net_ip_assignments (realm_id, expires);

CREATE OR REPLACE FUNCTION expand_cidr (p_cidr cidr, p_exclude inet[]) RETURNS inet[] LANGUAGE plpgsql AS $$
DECLARE
    r inet[] := '{}';
    latest inet := p_cidr;
    excl text[] := ARRAY(SELECT host(e) FROM unnest(p_exclude) e WHERE e IS NOT NULL);
BEGIN
    WHILE p_cidr >>= (latest + 2) LOOP
        latest = latest + 1;
        IF NOT (host(latest) = ANY (excl)) THEN
            r = r || latest;
        END IF;

    END LOOP;
    RETURN r;
END;
$$;

-- Probably don't use in any Rust because it will destroy performance
CREATE VIEW net_device_info AS
SELECT
  nd.id,
  nd.label,
  nd.hostname,
  c.mac,
  c.cred_id,
  c.network_id,
  COALESCE(nd.realm_id, nra.realm_id) AS realm_id,
  nd.last_updated,
  c.ipv4_addr
FROM
  net_devices nd
  LEFT JOIN net_realm_assignments nra ON nra.account_id = nd.account_id
  LEFT JOIN LATERAL (
    SELECT
      ARRAY_AGG(ndc.mac ORDER BY ndc.cred_id) AS mac,
      ARRAY_AGG(ndc.cred_id ORDER BY ndc.cred_id) AS cred_id,
      ARRAY_AGG(ndc.network_id ORDER BY ndc.cred_id) AS network_id,
      ARRAY_AGG(nia.ipv4_addr ORDER BY ndc.cred_id) AS ipv4_addr
    FROM net_device_creds ndc
      LEFT JOIN net_ip_assignments nia
        ON nia.device_id = ndc.device_id AND nia.cred_id = ndc.cred_id
    WHERE ndc.device_id = nd.id
  ) c ON true;
