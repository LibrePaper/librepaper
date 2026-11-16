-- The suggester rung: share links and grants may name 'suggester', which may
-- write only as tracked changes. The initial catalog named the roles inline,
-- and Postgres names those checks after their table and column, so each is
-- replaced rather than edited in place. grants.role stays nullable: a grant
-- that came through a share link keeps NULL here.
ALTER TABLE share_links DROP CONSTRAINT IF EXISTS share_links_role_check;
ALTER TABLE share_links ADD CONSTRAINT share_links_role_check
    CHECK (role IN ('reader','commenter','suggester','editor'));

ALTER TABLE grants DROP CONSTRAINT IF EXISTS grants_role_check;
ALTER TABLE grants ADD CONSTRAINT grants_role_check
    CHECK (role IN ('reader','commenter','suggester','editor'));
