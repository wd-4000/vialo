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
  ssid text, -- WiFi SSID to use in mobileconfig (NULL = use label)
  tls_trust text, -- TLS trusted server name for EAP (NULL = use org domain)
  outer_identity text -- EAP outer identity (NULL = anonymous@org.domain)
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
  last_updated timestamptz
);

CREATE INDEX net_cred_idx ON net_cred (network_id, username, password);

-- Realms are collections of subnet, nat and ipv6 prefix.
CREATE TABLE net_realms (
  id uuid PRIMARY KEY default gen_random_uuid(),
  ipv4_subnet cidr, -- If the cidr is /32 and the nat is null, the device is Un-Natted.
  ipv4_nat inet,
  ipv4_router inet,
  ipv4_dns inet,
  ipv6_prefix cidr,
  vlan int
);

-- Realm validation + IP pool management
CREATE OR REPLACE FUNCTION sync_realm_ip () RETURNS TRIGGER AS $$
BEGIN
    IF (TG_OP = 'INSERT' OR TG_OP = 'UPDATE') AND NEW.ipv4_subnet IS NOT NULL THEN
        -- Make sure we don't overlap
        IF ((SELECT EXISTS (SELECT 1 FROM net_realms nr WHERE NEW.ipv4_subnet >>= nr.ipv4_subnet AND id != NEW.id)) OR (SELECT EXISTS (SELECT 1 FROM net_ip_assignments WHERE NEW.ipv4_subnet >>= ipv4_addr AND realm_id != NEW.id))) THEN
            RAISE EXCEPTION 'overlap';
        END IF;

        -- Insert/update IP entries. Existing assignments get thrown out.
        SET CONSTRAINTS net_ip_assignments_realm_id_fkey DEFERRED;
        INSERT INTO net_ip_assignments (ipv4_addr, realm_id) VALUES (unnest(expand_cidr(NEW.ipv4_subnet, ARRAY[NEW.ipv4_router, NEW.ipv4_dns])), NEW.id) ON CONFLICT (ipv4_addr)
        DO UPDATE SET (realm_id, device_id) = (NEW.id, NULL);
    ELSIF TG_OP = 'DELETE' OR NEW.ipv4_subnet IS NULL THEN
        DELETE FROM net_ip_assignments WHERE realm_id = OLD.id;
        RETURN OLD;
    END IF;

    RETURN NEW;
END;
$$ LANGUAGE plpgsql;

create trigger tg_sync_realm_ip before insert
or
update
or delete on net_realms for each row
execute FUNCTION sync_realm_ip ();

CREATE TABLE net_realm_assignments (
  account_id uuid NOT NULL REFERENCES accounts (id) ON DELETE CASCADE, -- TODO/KEEPTODO: Make sure this is handled gracefully and in a legally compliant manner.
  realm_id uuid NOT NULL REFERENCES net_realms (id) ON DELETE CASCADE,
  PRIMARY KEY (account_id)
);

-- Network devices
CREATE TABLE net_devices (
  id uuid PRIMARY KEY default gen_random_uuid(),
  label text,
  hostname text,
  mac macaddr UNIQUE,
  cred_id uuid REFERENCES net_cred (id) ON DELETE CASCADE,
  realm_id uuid REFERENCES net_realms (id) ON DELETE SET NULL, -- Override for the user realm_id
  last_updated timestamptz default NOW(),
  last_seen timestamptz
);

CREATE INDEX idx_net_devices_cred ON net_devices (cred_id);

CREATE INDEX idx_net_devices_realm ON net_devices (realm_id);

-- IP address assignments
CREATE TABLE net_ip_assignments (
  ipv4_addr inet PRIMARY KEY,
  realm_id uuid NOT NULL REFERENCES net_realms (id) ON DELETE CASCADE DEFERRABLE INITIALLY IMMEDIATE, -- Realm this IP resides in. Essentially the IP pool.
  device_id uuid UNIQUE REFERENCES net_devices (id) ON DELETE CASCADE, -- Device that this IP is currently occupied by
  expires timestamptz -- Lease expiration date and time
);

CREATE OR REPLACE FUNCTION expand_cidr (p_cidr cidr, p_exclude inet[]) RETURNS inet[] LANGUAGE plpgsql AS $$
DECLARE
    r inet[];
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

CREATE VIEW net_device_info AS
SELECT
  nd.id,
  nd.label,
  nd.hostname,
  nd.mac,
  nd.cred_id,
  nc.network_id,
  COALESCE(nd.realm_id, nra.realm_id) AS realm_id,
  nd.last_updated,
  nia.ipv4_addr
FROM
  net_devices nd
  JOIN net_cred nc ON nc.id = nd.cred_id
  LEFT JOIN net_realm_assignments nra ON nc.account_id = nra.account_id
  LEFT JOIN net_ip_assignments nia ON nd.id = nia.device_id;
