CREATE TABLE home_quicklinks (
  id int PRIMARY KEY GENERATED ALWAYS AS IDENTITY,
  label int REFERENCES i18n_index (id) NOT NULL,
  link text NOT NULL
);

CREATE TABLE home_jumbo (
  id int PRIMARY KEY GENERATED ALWAYS AS IDENTITY,
  img text,
  headline int REFERENCES i18n_index (id),
  title int REFERENCES i18n_index (id),
  content int REFERENCES i18n_index (id),
  link text
);

CREATE OR REPLACE FUNCTION get_i18n_quicklinks (
  lang_priority TEXT[] -- Array of language labels, e.g., ['en', 'es', 'fr']
) RETURNS TABLE (id int, label text, link text) LANGUAGE plpgsql AS $$
BEGIN
    RETURN QUERY
    SELECT
        ql.id,
        get_i18n_string(ql.label, lang_priority) AS label,
        ql.link
    FROM
        home_quicklinks ql;
END;
$$;

CREATE OR REPLACE FUNCTION get_i18n_jumbo (
  lang_priority TEXT[] -- Array of language labels, e.g., ['en', 'es', 'fr']
) RETURNS TABLE (
  id int,
  img text,
  headline text,
  title text,
  content text,
  link text
) LANGUAGE plpgsql AS $$
BEGIN
    RETURN QUERY
    SELECT
        jb.id,
        jb.img,
        get_i18n_string(jb.headline, lang_priority) AS headline,
        get_i18n_string(jb.title, lang_priority) AS title,
        get_i18n_string(jb.content, lang_priority) AS content,
        jb.link
    FROM
        home_jumbo jb;
END;
$$;
