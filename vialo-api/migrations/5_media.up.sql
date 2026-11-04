--
-- Media
--
CREATE TYPE media_type AS ENUM('image', 'document');

CREATE TYPE media_context AS ENUM('post', 'avatar');

CREATE TYPE media_status AS ENUM('pending', 'ready', 'error');

-- files live in <media.dir>/<id[0..2]>/<id>/
CREATE TABLE media (
  id uuid PRIMARY KEY,
  type media_type NOT NULL,
  context media_context NOT NULL,
  mime text NOT NULL,
  filename text, -- type=document: download name
  size_bytes bigint NOT NULL,
  owner_id uuid NOT NULL REFERENCES accounts_people (id) ON DELETE CASCADE,
  status media_status NOT NULL DEFAULT 'pending',
  -- media_context=post: post the file is attached to
  post_id int REFERENCES board_posts (id) ON DELETE CASCADE,
  position int NOT NULL DEFAULT 0,
  -- type=image: set after processing, oriented
  width int,
  height int,
  variants int[], -- encoded widths
  created_at timestamptz NOT NULL DEFAULT NOW(),
  CHECK (type = 'image' OR (width IS NULL AND height IS NULL AND variants IS NULL)),
  CHECK (type = 'document' OR variants IS NOT NULL),
  CHECK (type = 'image' OR filename IS NOT NULL),
  CHECK (context = 'post' OR post_id IS NULL),
  CHECK (context <> 'avatar' OR type = 'image')
);

CREATE INDEX media_post_id_idx ON media (post_id, position)
WHERE
  post_id IS NOT NULL;

CREATE INDEX media_owner_id_idx ON media (owner_id);

ALTER TABLE accounts_people
ADD CONSTRAINT accounts_people_avatar_id_fkey FOREIGN KEY (avatar_id) REFERENCES media (id) ON DELETE SET NULL;

ALTER TABLE board_posts
ADD COLUMN cover_id uuid REFERENCES media (id) ON DELETE SET NULL;
