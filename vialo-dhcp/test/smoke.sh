#!/usr/bin/env bash
# Unit smoke test for vialo-dhcp.
#
# Boots the service under its real systemd unit (debian/service) — DynamicUser +
# ambient CAP_NET_BIND_SERVICE/CAP_NET_RAW + the full sandbox — inside a privileged
# Debian+systemd container, and asserts it:
#   1. reaches `active`,
#   2. binds UDP/67 (proves CAP_NET_BIND_SERVICE, and CAP_NET_RAW via SO_BINDTODEVICE
#      since `interfaces` pins a single interface),
#   3. connected to Postgres (the process panics on startup otherwise).
#
# Uses the existing dev `vialo` DB read-only: the service only opens connections and
# sets a session GUC; it writes nothing unless it processes a packet. No seeding, no
# cleanup, no dedicated test DB.
set -euo pipefail
cd "$(dirname "$0")/.."   # -> vialo/vialo-dhcp

# Same credentials the compose stack uses, rather than a copy that goes stale.
set -a && . ../../.env && set +a
DB_URL="postgresql://${POSTGRES_ROOT_USERNAME}:${POSTGRES_ROOT_PASSWORD}@postgres:5432/vialo"
dc() { docker compose "$@"; }
ex() { docker compose exec -T systemd "$@"; }

echo "==> Building vialo-dhcp binary"
dc up -d build >/dev/null
dc exec -T build cargo build -p vialo-dhcp

echo "==> Booting systemd container"
dc up -d --build systemd >/dev/null
for _ in $(seq 1 30); do
  state=$(ex systemctl is-system-running 2>/dev/null || true)
  case "$state" in running | degraded) break ;; esac
  sleep 1
done
echo "    systemd state: ${state:-unknown}"

echo "==> Installing unit, config, env file and binary"
docker compose exec -T -e DB_URL="$DB_URL" systemd bash -euo pipefail -c '
  install -Dm755 /workspace/vialo/target/debug/vialo-dhcp /usr/bin/vialo-dhcp
  install -Dm644 /workspace/vialo/vialo-dhcp/debian/service /etc/systemd/system/vialo-dhcp.service
  mkdir -p /etc/vialo
  siaddr=$(ip -4 -o addr show eth0 | awk "{print \$4}" | cut -d/ -f1)
  # 0644: the service runs under DynamicUser, so the config has to be readable
  # by a UID we do not know in advance. Secrets stay in dhcp.env below.
  printf "[dhcp]\nsiaddr = \"%s\"\ninterfaces = [\"eth0\"]\n" "$siaddr" >/etc/vialo/vialo.toml
  chmod 644 /etc/vialo/vialo.toml
  # No VIALO_CONFIG here on purpose: the unit points at /etc/vialo/vialo.toml
  # itself, and this test should catch it if that stops being true.
  umask 077
  printf "DATABASE_URL=%s\n" "$DB_URL" >/etc/vialo/dhcp.env
  systemctl daemon-reload
'

echo "==> Starting vialo-dhcp"
ex systemctl restart vialo-dhcp || true
sleep 2

echo
echo "================ results ================"
active=$(ex systemctl is-active vialo-dhcp 2>/dev/null || true)
# Local Address:Port is column 4; SO_BINDTODEVICE renders it as 0.0.0.0%eth0:67.
listen=$(ex ss -H -uln 2>/dev/null | awk '{print $4}' | grep -c ':67$' || true)
echo "is-active:        $active"
echo "listening on :67: $([ "$listen" -ge 1 ] && echo yes || echo no)"
echo "--- journal ---"
ex journalctl -u vialo-dhcp -n 25 --no-pager || true
echo "========================================="

if [ "$active" = active ] && [ "${listen:-0}" -ge 1 ]; then
  echo "SMOKE PASS: unit is active and bound to UDP/67 under its full sandbox."
  exit 0
else
  echo "SMOKE FAIL: see journal above."
  exit 1
fi
