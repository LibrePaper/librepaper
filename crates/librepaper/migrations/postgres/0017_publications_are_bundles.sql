-- A publication is a bundle, and always was.
--
-- Publishing used to be something a person did: they finished, they pressed
-- Publish, and readers got what they had pressed it on. That act is gone --
-- the CLI command with it -- and what a reader is shown now follows the
-- source on its own. What stayed is the thing the act produced: the rendered
-- HTML and the display assets it references, named by their digests, swapped
-- in whole. The manifest has called that a bundle all along (`bundle_sha256`);
-- every other name for it was the name of an act nobody performs.
--
-- So: `publications` is `bundles`, its files are `bundle_files`, and the
-- columns that named a publisher name whoever's browser did the rendering,
-- which is all they ever recorded.
--
-- Constraint and index names are left as PostgreSQL rewrote them. They are
-- named nowhere in the tree -- no query pins one by name -- and renaming them
-- would be a second migration's worth of churn for a string nobody reads.
ALTER TABLE publications RENAME TO bundles;
ALTER TABLE bundles RENAME COLUMN publisher_account_id TO rendered_by_account_id;
ALTER TABLE bundles RENAME COLUMN publisher_label TO rendered_by_label;

ALTER TABLE publication_files RENAME TO bundle_files;
ALTER TABLE bundle_files RENAME COLUMN publication_id TO bundle_id;

ALTER TABLE documents RENAME COLUMN current_publication_id TO current_bundle_id;

-- What a reader's remark was made against. A comment on a rendered page has
-- to name the page it was rendered from, and that page is a bundle.
ALTER TABLE annotations RENAME COLUMN publication_id TO bundle_id;
