-- Owner-issued agent tokens live beside share links. A token row is never read
-- as a link: `kind` keeps the two apart, so a token key presented as a browser
-- link matches nothing. `owner_generation` is the owner's session generation at
-- mint, and a token stops working when the owner's generation moves on.
ALTER TABLE share_links
    ADD COLUMN kind text NOT NULL DEFAULT 'link' CHECK (kind IN ('link','agent')),
    ADD COLUMN owner_generation text NOT NULL DEFAULT '';
