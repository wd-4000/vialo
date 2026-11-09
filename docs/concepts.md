# Concepts
## Accounts & Groups

1. **Identity**: the login side of an account.
   Vialo integrates with [Ory Kratos](https://github.com/ory/kratos) for authentication.
   When someone signs up or changes their profile in Kratos, a webhook pushes the updated
   identity to vialo's hooks API, keeping the local copy in sync.

2. **Accounts** represent a singular person or a group.

   a) **Person**: person that hasn't necessarily signed up.
   They can have a name, email, phone number, and membership status.
   Each account can belong to one or more groups.
   Normally bound to an Identity.

   b) **Group**: a named collection of accounts, like a floor
   or a working group (Arbeitsgruppe/AG). A group can be public (visible to everyone) or
   private. Groups have:

    - **Members** (of People) with **roles** (manager or member). Managers can edit group
      settings (except app roles).
    - **App roles**: what the group's members are allowed to do globally in vialo
      (manage all Bookables, edit Networks, view the Health dashboard, etc.).
    - An optional **email address** used as a mailing list and the sender for
      group-specific notifications.


## Networks
vialo models a network in three layers:

1. **Network**: a specific access method. A WiFi SSID or a wired Ethernet
segment. This is what users connect to.

A network has an **auth** mode: `username_password` - RADIUS, `password` PPSK or 
`null` (none).

2. **Realm**: a reusable IP networking configuration. A VLAN ID, an IPv4
subnet, and NAT settings.

NAT is included as more of a convenience & is not synced anywhere. So, the actual
subnet -> external IP mapping has to be done separately on the router.

3. **Credential**: an "instance" of a network, if you will. Every credential is bound to a specific Account. 
   vialo generates and tracks these. For a `username_password` network, that's a combination 
   of a username and a password, for `password` – the password. They also exist for no-auth networks.
   There, they are just the "instance" of the network for the user.


### DHCP
Vialo runs its own DHCP server that automatically assigns IP addresses
to registered devices. It reads the realm configuration (subnet, router, DNS)
and hands out leases from that realm's IP pool. Devices are identified by MAC
address. VLAN hints from relay agents or switch port configuration narrow down the
lookup to the correct realm.

## Bookables

A **bookable** is anything that can be reserved for a time slot: a washing
machine, a common room, a 3D printer. Each bookable has:

- **Asset type**: a kind of asset. For example, "Washing Machine".
- **Asset**: a concrete instance of a type. I.e. "Washing Machine 3"
- **Schema**: the booking slots and pricing for a specific type.
  So, let's say we have schedule entries for 10:00, 11:00, and 11:30.
  One can then book an appointment for the 10:00-11:00 slot, and the 11:00-11:30 one.
  Prices are currently uniform for every slot. The schema also stores refund rules.

  Schemas are assigned per-asset with a beginning date.

- **Connector**: an optional physical device backing the asset, like a NetIO
  smart outlet that powers the washing machine on when a booking starts or whatever.

Bookings are **appointments**. When an appointment expires, credits may be
refunded depending on the currently active schema's settings.

## Credits

Credits are vialo's internal currency. They are spent on bookable reservations
and printer usage.

Admins set credit balances manually through the admin UI.

- **Ledger**: every credit transaction (spend, refund, adjustment) is recorded
  in the credit ledger. This is an append‑only audit trail.
- **Products**: transactions are labeled by what they paid for. `printer_bw`,
  `printer_color`, or a specific bookable.

## Posts & Boards

A lightweight community content system:

- **Board**: where posts are posted. Can belong to a group, making any notifications come
  from that group's email.
- **Post**: an announcement or event on a board. Has a title, description,
  optional event timespan and location.
- **Subscription**: accounts can subscribe to boards they belong to. When a
  new post is published, subscribers get an email notification.

## Subsystems

Subsystems are vialo's background workers. Each one runs as an async task
alongside the HTTP listeners, processing a job queue stored in the
`subsystem_jobs` table:

| Subsystem | What it does |
|---|---|
| **App** | Generic value for "done by the app" |
| **Bookable** | Handles bookable activation, appointment expiry, connector sync |
| **Printer** | Creates/deletes printer users, sets print limits, commits credit deductions. |
| **NMS** | Pushes PPSK Wi‑Fi credentials to a network management system and syncs VLANs |
| **Email** | Sends notification emails |
| **Sysop** | Sign-in for people directly changing the database |
| **DHCP** | DHCP server. A standalone binary (`vialo-dhcp`) |
| **Import** | Imported |
| **Media** | Media encoding |


Subsystems are identified by name in the audit log. Every history entry is
attributed either to a Person or to a Subsystem.
