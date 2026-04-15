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
  active boolean NOT NULL DEFAULT true -- New devices/credentials may be added
);

CREATE TYPE net_nms_type AS ENUM('unifi', 'mikrotik');

CREATE TABLE net_nms_connectors (
  id int PRIMARY KEY GENERATED ALWAYS AS IDENTITY,
  type net_nms_type NOT NULL,
  hostname text NOT NULL,
  api_key text,
  site_id text,
  username text,
  password text
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
  password text,
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

CREATE OR REPLACE FUNCTION radius_auth (
  r_username text,
  r_password text,
  r_network_id int,
  r_mac macaddr
) RETURNS TABLE (vlan int) LANGUAGE plpgsql AS $$
DECLARE
  f_vlan int;
  f_vlan_override int;
  f_cred_id uuid;
  f_add_on_auth boolean;
  f_multi_device boolean;
  f_existing_device_id uuid;
BEGIN
    -- Check that we pass authentication & get some required variables
     SELECT nr.vlan, nc.id, nt.auto_add_on_auth, nt.multi_device INTO f_vlan, f_cred_id, f_add_on_auth, f_multi_device FROM net_cred nc
        JOIN net_networks nt ON nc.network_id = nt.id
        LEFT JOIN net_realm_assignments nra ON nc.account_id = nra.account_id
        LEFT JOIN net_realms nr ON nr.id = nra.realm_id
        WHERE nc.network_id = r_network_id AND nt.auth = 'username_password'::net_auth AND nc.username = r_username AND nc.password = r_password LIMIT 1;

    IF f_cred_id IS NULL THEN
        RAISE EXCEPTION 'nonexistent_cred' USING DETAIL = format('username: %s', r_username);
    END IF;

    IF f_vlan IS NULL THEN
        RAISE EXCEPTION 'no_vlan_assigned' USING DETAIL = format('cred_id: %s', f_cred_id);
    END IF;

     SELECT nr.vlan INTO f_vlan_override FROM net_devices nd JOIN net_realms nr ON nd.realm_id = nr.id WHERE nd.mac = r_mac;
    -- Update device (if needed)
    IF f_add_on_auth THEN
        IF NOT f_multi_device THEN
            -- Singular device per credential mode
            SELECT id INTO f_existing_device_id FROM net_devices WHERE cred_id = f_cred_id;
        ELSE
            -- There's a free slot for a multi-device credential
            SELECT id INTO f_existing_device_id FROM net_devices WHERE cred_id = f_cred_id AND mac IS NULL;
        END IF;

        IF f_existing_device_id IS NOT NULL THEN
            UPDATE net_devices SET (cred_id, mac, hostname) = (f_cred_id, r_mac, NULL) WHERE id = f_existing_device_id;
        ELSE
            INSERT INTO net_devices (cred_id, mac) VALUES (f_cred_id, r_mac) ON CONFLICT (mac)
            DO UPDATE SET (cred_id, mac, hostname) = (f_cred_id, r_mac, NULL);
        END IF;
    END IF;

    RETURN QUERY SELECT COALESCE(f_vlan_override, f_vlan) AS vlan;
END;
$$;

CREATE OR REPLACE FUNCTION radius_dhcp (r_mac macaddr, r_vlan int) RETURNS TABLE (ipv4_addr inet, ipv4_subnet cidr) LANGUAGE plpgsql AS $$
DECLARE
    f_realm_id uuid;
    f_ipv4_subnet cidr;
    f_device_id uuid;
    f_ipv4_addr inet;
    -- TODO
    -- f_auto_add boolean;
    -- f_cred_id uuid;
BEGIN
    -- Check whether the device has been registered
    SELECT ndi.realm_id, ndi.id, nr.ipv4_subnet INTO f_realm_id, f_device_id, f_ipv4_subnet FROM net_device_info ndi JOIN net_realms nr ON nr.id = ndi.realm_id
        WHERE ndi.mac = r_mac AND nr.vlan = r_vlan;

    -- TODO: No? Maybe auto-add it?
    IF f_realm_id IS NULL OR f_device_id IS NULL THEN
        -- SELECT auto_add_via_dhcp INTO f_auto_add FROM net_networks WHERE id = r_network_id;
        -- IF f_auto_add THEN
        --     SELECT id INTO f_cred_id FROM net_cred WHERE network_id = r_network_id AND LIMIT 1;

        --     INSERT INTO net_devices (cred_id, network_id, mac) VALUES (f_cred_id, r_network_id, r_mac) ON CONFLICT (mac)
        --     DO UPDATE SET (cred_id, network_id, mac, hostname, label) = (f_cred_id, r_network_id, r_mac, NULL,NULL);
        -- ELSE
            RAISE EXCEPTION 'unknown_device';
        -- END IF;
    END IF;

    -- Check if we actually already have an IP for this device
    SELECT nia.ipv4_addr INTO f_ipv4_addr FROM net_ip_assignments nia WHERE nia.realm_id = f_realm_id AND nia.device_id = f_device_id LIMIT 1;

    -- Or find a new one, I guess
    IF f_ipv4_addr IS NULL THEN
        SELECT nia.ipv4_addr INTO f_ipv4_addr FROM net_ip_assignments nia WHERE (nia.device_id IS NULL OR nia.expires <= NOW()) AND nia.realm_id = f_realm_id;
    END IF;

    IF f_ipv4_addr IS NULL THEN
        RAISE EXCEPTION 'pool_full';
    END IF;

    UPDATE net_ip_assignments nia SET (device_id, expires) = (f_device_id, NOW()+'3 hours') WHERE nia.ipv4_addr = f_ipv4_addr;
    UPDATE net_devices SET last_seen = NOW() WHERE id = f_device_id;

    RETURN QUERY SELECT f_ipv4_addr AS ipv4_addr, f_ipv4_subnet AS ipv4_subnet;
END;
$$;

CREATE OR REPLACE FUNCTION expand_cidr (p_cidr cidr, p_exclude inet[]) RETURNS inet[] LANGUAGE plpgsql AS $$
DECLARE
    r inet[];
    latest inet := p_cidr;
BEGIN
    WHILE p_cidr >>= (latest + 2) LOOP
        latest = latest + 1;
        IF array_position(p_exclude, latest) IS NULL THEN
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
