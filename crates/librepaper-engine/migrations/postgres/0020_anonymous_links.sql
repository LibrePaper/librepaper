-- A link may be held without signing in, when its owner marks it so and the
-- deployment's `[access]` switch for its role is on. Existing links keep
-- requiring a sign-in.
ALTER TABLE share_links ADD COLUMN anonymous boolean NOT NULL DEFAULT false;
