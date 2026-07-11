# Vialo Concepts

A plain-English explainer of the nouns in vialo. What each thing is and how
they relate. No implementation details, no config syntax. If you're reading
code or configuring a deployment, start here.

## Accounts & Groups

**Account**: a person with a login. Has a name, email, phone number, and
membership status. Each account can belong to one or more groups.

**Group** (Arbeitsgruppe / AG): a named collection of accounts, like a dorm
wing or a student committee. A group can be public (visible to everyone) or
private. Groups have:

- **Members** with **roles** (manager or member). Managers can edit group
  settings and moderate content.
- **App permissions**: what the group's members are allowed to do in vialo
  (manage bookables, edit networks, view the health dashboard, etc.).
- An optional **email address** used as the sender for group-specific
  notifications.

**Identity**: the login side of an account. vialo itself does not handle
passwords; it integrates with [Ory Kratos](https://www.ory.sh/kratos/) for
authentication. When someone signs up or changes their profile in Kratos, a
webhook pushes the updated identity to vialo's hooks API, keeping the local
account in sync.

## Networks

vialo models a network in three layers:

**Network**: a specific access method. A Wi‑Fi SSID or a wired Ethernet
segment. This is what users connect to.

**Realm**: a bundle of IP networking configuration. A VLAN ID, an IPv4 or IPv6
subnet, and NAT settings. Realms are reusable. Assign the same realm to
multiple networks and they share the same addressing and routing.

A network has an **auth** mode: `password` (per‑user credentials via
RADIUS/PPSK) or `open` (no authentication).

**Credential**: a per-account password for a password-protected network. vialo
generates and tracks these. For Wi‑Fi networks they become PPSK (Private
Pre‑Shared Key) entries pushed to the UniFi controller. Credentials are tied to
a specific account, a specific network, and optionally a realm.

**DHCP**: vialo runs its own DHCP server that automatically assigns IP addresses
to registered devices. It reads the realm configuration (subnet, router, DNS)
and hands out leases from that realm's IP pool. Devices are identified by MAC
address. VLAN hints from relay agents or switch port configuration scope the
lookup to the correct realm.

## Bookables

A **bookable** is anything that can be reserved for a time slot: a washing
machine, a common room, a 3D printer. Each bookable has:

- **Asset type**: a template defining schedules, pricing (in credits), and
  booking rules. For example, "Washing Machine" with a schedule of 2‑hour
  slots at 1 credit each.
- **Asset**: a concrete instance of a type. "Washing Machine 3 in Basement A."
- **Connector**: an optional physical device backing the asset, like a NetIO
  smart outlet that powers the washing machine on when a booking starts.

Bookings are **appointments**. When an appointment expires, credits may be
refunded automatically if the asset wasn't used.

## Credits

Credits are vialo's internal currency. They are spent on bookable reservations
and printer usage.

- **Allocation**: admins set credit balances manually through the admin UI.
  Credits are not earned or purchased through the app itself.
- **Ledger**: every credit transaction (spend, refund, adjustment) is recorded
  in the credit ledger. This is an append‑only audit trail.
- **Products**: transactions are labelled by what they paid for. `printer_bw`,
  `printer_color`, or a specific bookable.

## Posts & Boards

A lightweight community content system:

- **Board**: belongs to a group. Think of it as a bulletin board for that
  group.
- **Post**: an announcement or event on a board. Has a title, description
  (HTML and plain text), optional event timespan, and pinned status.
- **Subscription**: accounts can subscribe to boards they belong to. When a
  new post is published, subscribers get an email notification.

## Subsystems

Subsystems are vialo's background workers. Each one runs as an async task
alongside the HTTP listeners, processing a job queue stored in the
`subsystem_jobs` table:

| Subsystem | What it does |
|---|---|
| **Printer** | Syncs user accounts and page counters with a Konica Minolta printer. Creates/deletes printer users, sets print limits, commits credit deductions. |
| **PPSK** | Pushes per‑account Wi‑Fi credentials to a UniFi controller so they appear as PPSK entries on the Wi‑Fi network. |
| **Email** | Listens for post and appointment events and sends notification emails. |
| **Bookables** | Manages connector devices (e.g., NetIO outlets). Powers them on/off when bookings start/end. |
| **DHCP** | Standalone binary (`vialo-dhcp`) that answers DHCP Discover, Request, Release, and Decline for registered devices. Assigns IPs from realm pools, extends leases, quarantines declined addresses, and updates device last-seen timestamps. |

Subsystems are identified by name in the audit log. Every history entry is
attributed either to an account or to a subsystem.
