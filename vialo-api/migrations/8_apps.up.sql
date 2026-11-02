CREATE TABLE apps (
  id int PRIMARY KEY GENERATED ALWAYS AS IDENTITY,
  label int REFERENCES i18n_index (id) NOT NULL,
  url text NOT NULL,
  icon text,
  index int NOT NULL DEFAULT 0, -- display order
  captive bool NOT NULL DEFAULT false -- todo
);

CREATE TABLE app_group_perms (
  id int PRIMARY KEY GENERATED ALWAYS AS IDENTITY,
  group_id uuid REFERENCES account_groups (id) ON DELETE CASCADE, -- NULL means public
  app_id int REFERENCES apps (id) ON DELETE CASCADE NOT NULL,
  UNIQUE NULLS NOT DISTINCT (group_id, app_id)
);
