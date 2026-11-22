-- vialo-dhcp connects as its own role (created by `vialo db init`) with only what
-- it uses. Skipped where that role doesn't exist, e.g. in Docker.
DO $$
BEGIN
  IF EXISTS (SELECT FROM pg_roles WHERE rolname = 'vialo-dhcp') THEN
    GRANT SELECT ON net_realms, net_realm_assignments, net_devices, net_device_creds,
      net_ip_assignments TO "vialo-dhcp";
    GRANT UPDATE (device_id, cred_id, expires) ON net_ip_assignments TO "vialo-dhcp";
    GRANT UPDATE (last_seen) ON net_devices TO "vialo-dhcp";
    -- net_devices' history trigger runs with the caller's privileges
    GRANT INSERT ON table_log TO "vialo-dhcp";
  END IF;
END $$;
