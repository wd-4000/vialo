CREATE TABLE health_events (
  id int PRIMARY KEY GENERATED ALWAYS AS IDENTITY,
  created_at timestamptz DEFAULT NOW() NOT NULL,
  last_updated timestamptz,
  subsystem subsystem_type,
  label text NOT NULL,
  data jsonb,
  badness int DEFAULT 0 NOT NULL,
  read boolean DEFAULT false NOT NULL,
  resolved boolean DEFAULT false NOT NULL
);

CREATE INDEX idx_health_events_unresolved_unread ON health_events (badness)
WHERE
  resolved = false
  AND read = false;

CREATE INDEX idx_health_events_time_diff ON health_events ((last_updated - created_at))
WHERE
  resolved = false
  AND read = false;
