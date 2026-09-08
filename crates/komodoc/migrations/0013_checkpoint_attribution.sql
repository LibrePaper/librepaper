-- Checkpoint attribution used to be the mutable display handle in `by`, while
-- erasure matched stable account ids.  A renamed or reused handle therefore
-- could not identify an account's historical contributions, and erasure could
-- miss rows or target the wrong ones.
--
-- `by_account` carries the stable provider id (`accounts.id`) of the
-- authenticated caller, separately from the display string.  It is nullable
-- because anonymous, imported, system and automatic checkpoints have no
-- authoritative account association, and because existing rows have none:
-- there is no record from which a legacy row's account could be recovered,
-- and inferring one from `by` would be a handle match.
--
-- There is deliberately no foreign key to `accounts`.  A checkpoint may be
-- written in a deployment whose identities are not catalogued at all, and the
-- decision to drop attribution belongs to the erasure protocol rather than to
-- a cascade that could clear it as a side effect of any account row removal.
ALTER TABLE checkpoints ADD COLUMN by_account TEXT;

-- The erasure worker walks one account's checkpoints in bounded keyset
-- batches ordered by (slug, sha).  The partial index keeps that walk off the
-- table for attributed rows and costs nothing for the NULL majority.
CREATE INDEX checkpoints_by_account
    ON checkpoints (by_account, slug, sha)
    WHERE by_account IS NOT NULL;
