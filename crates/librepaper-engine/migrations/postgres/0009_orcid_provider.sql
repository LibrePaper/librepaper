-- ORCID is a built-in sign-in provider, so refusals may name it too.
ALTER TABLE banned_handles DROP CONSTRAINT banned_handles_provider_check;
ALTER TABLE banned_handles ADD CONSTRAINT banned_handles_provider_check
    CHECK (provider ~ '^(github|google|entra|orcid|oidc-[a-z][a-z0-9-]{0,23})$');
