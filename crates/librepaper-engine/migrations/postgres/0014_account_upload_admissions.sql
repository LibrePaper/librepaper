-- One row for each admitted upload request. The account id makes the limit
-- survive browser changes, sign-in sessions, and server restarts.
CREATE TABLE account_upload_admissions (
    id bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    account_id uuid NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
    created_at timestamptz NOT NULL DEFAULT now()
);

CREATE INDEX account_upload_admissions_recent
    ON account_upload_admissions(account_id, created_at DESC);
CREATE INDEX account_upload_admissions_created
    ON account_upload_admissions(created_at);
