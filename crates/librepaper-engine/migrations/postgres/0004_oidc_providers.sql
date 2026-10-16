-- Each OIDC provider id is bound to the issuer it was first started with.
-- A subject is unique only within its issuer, so pointing an existing id at
-- another issuer would hand old accounts to whoever holds the same subject
-- there. The server refuses to start instead.
CREATE TABLE sign_in_issuers (
    provider text PRIMARY KEY,
    issuer text NOT NULL,
    bound_at timestamptz NOT NULL DEFAULT now()
);

-- Refusals may name any provider id the server accepts.
ALTER TABLE banned_handles DROP CONSTRAINT banned_handles_provider_check;
ALTER TABLE banned_handles ADD CONSTRAINT banned_handles_provider_check
    CHECK (provider ~ '^(github|google|entra|oidc-[a-z][a-z0-9-]{0,23})$');
