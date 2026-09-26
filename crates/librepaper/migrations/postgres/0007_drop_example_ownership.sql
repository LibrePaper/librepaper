-- Remove the example ownership mode.
--
-- The public front-page listing and the reserved examples account are
-- being removed. Every new account gets its own private starter documents
-- at sign-in (see server/onboarding.rs), so the shared "examples" are no
-- longer needed. Example documents are converted to owned.

UPDATE documents SET ownership_mode='owned' WHERE ownership_mode='example';

DROP INDEX documents_examples_updated;

ALTER TABLE documents DROP CONSTRAINT documents_ownership_mode_check;

ALTER TABLE documents
    ADD CONSTRAINT documents_ownership_mode_check
    CHECK (ownership_mode IN ('owned', 'open'));
