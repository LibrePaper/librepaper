-- Successful account initiated project and figure uploads consume the same
-- rolling hourly allowance. Events outlive replacement or deletion of the
-- project rows whose bytes they admitted.
CREATE TABLE upload_admissions (
    id uuid PRIMARY KEY,
    account_id uuid NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
    created_at timestamptz NOT NULL DEFAULT now()
);

CREATE INDEX upload_admissions_account_created_at
    ON upload_admissions(account_id, created_at DESC);
