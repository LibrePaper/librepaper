-- Refusals may also name a generic OpenID Connect account, so the provider
-- list gains 'oidc'. The constraint keeps the name Postgres gave the inline
-- check in 0002, so the drop and the re-add name the same constraint.
ALTER TABLE banned_handles DROP CONSTRAINT banned_handles_provider_check;
ALTER TABLE banned_handles ADD CONSTRAINT banned_handles_provider_check
    CHECK (provider IN ('github', 'google', 'oidc'));
