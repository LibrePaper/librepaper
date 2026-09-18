//! Who else may see a document: links that carry a role, and handing the whole
//! document to somebody else.

use super::*;

/// The header a browser presents a link key on, and the query parameter the
/// socket carries it in -- the one place a browser cannot set a header. The
/// key itself never reaches a log: what is recorded anywhere is its hash.
pub const LINK_HEADER: &str = "x-librepaper-key";

pub const LINK_PARAM: &str = "k";

/// How long a new link lasts unless something shorter is asked for. A round of
/// review has an end, and a link that lives for ever is a leak waiting for a
/// forwarded email; the dialog offers to renew, which mints a new key.
pub const LINK_DEFAULT_SECONDS: i64 = 180 * 24 * 3600;

pub(super) const MAX_LINK_LABEL: usize = 80;

/// What one call to the share route asks for. Every field is optional, and
/// several may arrive together: a revoke and a mint in one round trip is a
/// rotation asked for the long way.
#[derive(Deserialize, Default)]
pub(super) struct ShareRequest {
    /// A role word (`read`/`reader`, `comment`/`commenter`, `edit`/`editor`).
    #[serde(default)]
    pub(super) revoke: Option<String>,
    #[serde(default)]
    pub(super) link: Option<LinkRequest>,
    /// Changes to the link a role already holds -- its memo, its expiry, its
    /// comment budget -- leaving the key alone. Minting is what kills a link,
    /// so an owner correcting a label is not made to hand out a new URL.
    #[serde(default)]
    pub(super) settings: Option<LinkRequest>,
}

#[derive(Deserialize, Default)]
pub(super) struct LinkRequest {
    #[serde(default)]
    pub(super) role: String,
    /// A duration such as `180d` or `24h`, `never` for a link that does not
    /// expire, or absent: absent keeps the expiry a link already has, and is
    /// the default for a link that is new.
    #[serde(default)]
    pub(super) until: String,
    /// A memo for the owner, not an identity or an additional grant. Omitted
    /// on rotation preserves the current label; an empty string clears it.
    #[serde(default)]
    pub(super) label: Option<String>,
    /// Comment actions per clock hour for this link. Omitted uses the
    /// deployment's ordinary comment limit.
    #[serde(default)]
    pub(super) budget: Option<i64>,
}

/// A change to the link a role already holds, once the request's words have
/// been read: the role it names, an expiry only where one was asked for, a memo
/// only where one was sent, and the budget as it stands.
struct LinkSettings {
    role: Role,
    until: Option<String>,
    label: Option<String>,
    budget: Option<i64>,
}

/// The word a link's role or a revoke may be spelled with. The document
/// stores the long form, but the dialog and the command line offer the short
/// verb too -- `read`, `comment`, `edit` -- because that is what somebody
/// asking to share a document actually types.
pub(super) fn parse_role_word(value: &str) -> Option<Role> {
    match value.trim().to_ascii_lowercase().as_str() {
        "read" | "reader" => Some(Role::Reader),
        "comment" | "commenter" => Some(Role::Commenter),
        "edit" | "editor" => Some(Role::Editor),
        _ => None,
    }
}

/// When a new link stops working. Absent means the default, `never` means it
/// does not expire, and anything else is a duration the retention flag already
/// knows how to read.
pub(super) fn link_expiry(asked: &str) -> Result<String, String> {
    let asked = asked.trim();
    if asked.eq_ignore_ascii_case("never") {
        return Ok(String::new());
    }
    let seconds = if asked.is_empty() {
        LINK_DEFAULT_SECONDS
    } else {
        crate::document::retention::parse_retention(asked)
            .map_err(|_| "an expiry is a duration such as 180d or 24h, or 'never'".to_string())?
    };
    if seconds <= 0 {
        return Err("an expiry is a duration such as 180d or 24h, or 'never'".to_string());
    }
    Ok(crate::util::format_unix(crate::util::now_unix() + seconds))
}

/// The provider namespace of a catalog account identity.
pub(super) fn provider_of(id: &str) -> String {
    id.split_once(':')
        .map(|(provider, _)| provider.to_string())
        .unwrap_or_default()
}

/// Only GitHub logins resolve to an account today: a grant to an email
/// address waits on the sharing spec's step that records a handle nobody has
/// signed in with yet. Until then an address is refused before anybody asks
/// GitHub about it, with a message that says what is missing.
pub(super) const EMAIL_GRANTS_UNAVAILABLE: &str =
    "sharing with an email address is not available yet; name a GitHub login";

/// Whether what was typed is an address rather than a login: a GitHub login
/// cannot contain `@` past the optional one in front.
pub(super) fn names_an_address(asked: &str) -> bool {
    asked.trim().trim_start_matches('@').contains('@')
}

/// Why a login could not be turned into an account.
pub(super) fn no_such_account(asked: &str) -> String {
    format!(
        "github has no account called @{}",
        clean(asked.trim().trim_start_matches('@'), 64)
    )
}

/// The digest a link key is known by. Empty in, empty out: no link at all is
/// not the same question as a link that does not match.
pub fn hash_link_key(key: &str) -> String {
    let key = key.trim();
    if key.is_empty() {
        return String::new();
    }
    hex::encode(Sha256::digest(key.as_bytes()))
}

/// A new link key: 256 bits from the same source every other secret here comes
/// from, well past the 128 the spec asks for. The document keeps its digest,
/// which is what admits a caller, and a sealed copy of the key itself, which
/// is what lets its owner be handed the URL again.
pub fn mint_link_key() -> String {
    hex::encode(crate::auth::random_bytes(32))
}

/// The key that seals a link key, from the deployment's session key. A purpose
/// string of its own keeps it off the key that signs sessions: the two are
/// derived from the same root and are used for different things.
fn sealing_key(session_key: &[u8]) -> chacha20poly1305::Key {
    let mut digest = Sha256::new();
    digest.update(b"librepaper-share-link-v1");
    digest.update(session_key);
    chacha20poly1305::Key::from(<[u8; 32]>::from(digest.finalize()))
}

/// A link key as the catalogue holds it: a nonce and the sealed bytes, so what
/// is written down is useless to anybody who does not also hold the
/// deployment's session key -- which is a file in the secrets directory, and
/// never in the catalogue. The digest goes on doing the whole job of admitting
/// a caller; this is only how the URL is said again.
pub fn seal_link_key(session_key: &[u8], key: &str) -> Vec<u8> {
    use chacha20poly1305::aead::{Aead, KeyInit};
    let cipher = chacha20poly1305::XChaCha20Poly1305::new(&sealing_key(session_key));
    let nonce = crate::auth::random_bytes(24);
    let Ok(sealed) = cipher.encrypt(chacha20poly1305::XNonce::from_slice(&nonce), key.as_bytes())
    else {
        return Vec::new();
    };
    [nonce, sealed].concat()
}

/// The key back, or nothing. Nothing is an ordinary answer: a link minted
/// before the catalogue kept the key at all has no sealed copy, and a
/// deployment whose session key was replaced can no longer read the ones it
/// has. Either way the link goes on working -- its digest is what admits
/// anybody -- and only the offer to copy its URL is missing.
pub fn open_link_key(session_key: &[u8], sealed: &[u8]) -> String {
    use chacha20poly1305::aead::{Aead, KeyInit};
    if sealed.len() <= 24 {
        return String::new();
    }
    let cipher = chacha20poly1305::XChaCha20Poly1305::new(&sealing_key(session_key));
    let (nonce, body) = sealed.split_at(24);
    match cipher.decrypt(chacha20poly1305::XNonce::from_slice(nonce), body) {
        Ok(key) => String::from_utf8(key).unwrap_or_default(),
        Err(_) => String::new(),
    }
}

impl Server {
    /// Mints this document's read link, replacing any it had, and returns the
    /// path to hand out. The key is generated before the write, as the share
    /// route does, since the write is what commits it.
    pub(super) async fn mint_read_link(
        &self,
        slug: &str,
        caller: &Caller,
    ) -> Result<String, ModifyError> {
        let key = mint_link_key();
        let link = LinkGrant {
            hash: hash_link_key(&key),
            role: Role::Reader.as_str().to_string(),
            sealed: seal_link_key(&self.key, &key),
            since: crate::util::timestamp(),
            ..Default::default()
        };
        let actor = crate::document::store::MutationActor {
            account_id: caller.id.clone(),
            owner_key: caller.key.clone(),
            session_generation: caller.session_generation.clone(),
            link_hash: String::new(),
            policy_editor: true,
            unowned_publisher: caller.id.is_empty(),
        };
        self.store
            .modify_as_owner(slug, &actor, |entry| {
                entry.set_link(link.clone());
                Ok(())
            })
            .await?;
        Ok(format!("/docs/{slug}#k={key}"))
    }

    /// Who a document is shared with, and -- for its owner -- the changes to
    /// that. Reading takes a place on the document by name, so a commenter can
    /// see who else is in the room; a reader who arrived by link is not shown
    /// the other reviewers, which is most of the point of a blind review.
    /// Writing takes the owner: an editor cannot share, because the owner is
    /// the one whose quota and whose name are on the document.
    pub(super) async fn handle_share(
        &self,
        request: Request<Body>,
        arrival: &Arrival,
        slug: &str,
    ) -> Reply {
        if !self.valid_slug(slug) {
            return write_json(400, &json!({"error": "bad slug"}));
        }
        let entry = match self.checked_entry(slug).await {
            Ok(Some(entry)) => entry,
            Ok(None) => return write_json(404, &json!({"error": "not found"})),
            Err(response) => return response,
        };
        let headers = request.headers().clone();
        let method = request.method().clone();
        let query = request.uri().query().map(str::to_string);
        let who = self
            .viewer(&entry, &headers, arrival, query.as_deref())
            .await;
        if who.auth_failed {
            return write_json(
                401,
                &json!({"error": "authentication expired or was revoked"}),
            );
        }
        if who.id.is_signed_in() && !self.provider_configured(&who.id) {
            return write_json(
                401,
                &json!({"error": "authentication provider is not configured"}),
            );
        }
        // The share route is the owner's alone now: a named editor used to be
        // shown a read-only version of it, but a document names its coauthors
        // through a link these days, and a stranger -- named or not -- learns
        // nothing from this route, not even that the document is there.
        if !who.at_least(Role::Owner) {
            return write_json(404, &json!({"error": "not found"}));
        }
        if method == Method::GET {
            return write_json(200, &self.sharing_json(&entry));
        }
        if method != Method::POST {
            return plain(405, "method not allowed");
        }
        if cross_site_refused(&headers, arrival) {
            return write_json(403, &cross_site_refusal());
        }

        let Ok(body) = to_bytes(request.into_body(), 1 << 16).await else {
            return write_json(400, &json!({"error": "bad request"}));
        };
        let Ok(asked) = serde_json::from_slice::<ShareRequest>(&body) else {
            return write_json(400, &json!({"error": "bad request"}));
        };
        let current_entry = match self.checked_entry(slug).await {
            Ok(Some(entry)) => entry,
            Ok(None) => return write_json(404, &json!({"error": "not found"})),
            Err(response) => return response,
        };
        let current_who = self
            .viewer(&current_entry, &headers, arrival, query.as_deref())
            .await;
        if current_who.auth_failed || !current_who.at_least(Role::Owner) {
            return write_json(403, &json!({"error": "ownership changed"}));
        }

        // A new link's key is minted before the write, because the write is
        // what commits it; whether it does anything for anybody is `role_of`'s
        // business from here on, so minting is never itself refused. This is
        // also the rotation: `set_link` below replaces whatever already
        // carried this role, so minting again is how a leaked link is killed
        // without losing the role it stood for.
        let mut minted: Option<LinkGrant> = None;
        if let Some(wanted) = &asked.link {
            let Some(role) = parse_role_word(&wanted.role) else {
                return write_json(
                    400,
                    &json!({"error": "a link is 'reader', 'commenter' or 'editor'"}),
                );
            };
            let until = match link_expiry(&wanted.until) {
                Ok(until) => until,
                Err(message) => return write_json(400, &json!({"error": message})),
            };
            if wanted.budget.is_some_and(|budget| budget < 0) {
                return write_json(
                    400,
                    &json!({"error": "a link budget is a non-negative number of comments per hour"}),
                );
            }
            let label = wanted
                .label
                .as_deref()
                .map(|label| clean(label, MAX_LINK_LABEL).trim().to_string());
            let key = mint_link_key();
            minted = Some(LinkGrant {
                hash: hash_link_key(&key),
                role: role.as_str().to_string(),
                sealed: seal_link_key(&self.key, &key),
                since: crate::util::timestamp(),
                until,
                label: label.unwrap_or_default(),
                budget: wanted.budget,
            });
        }

        // A settings change is the mint's opposite number: the same four
        // fields, read the same way, applied to the row already there. It is
        // checked out here, where a bad role or a bad duration can still be
        // answered with the reason rather than a refused mutation.
        let mut adjusted: Option<LinkSettings> = None;
        if let Some(wanted) = &asked.settings {
            let Some(role) = parse_role_word(&wanted.role) else {
                return write_json(
                    400,
                    &json!({"error": "a link is 'reader', 'commenter' or 'editor'"}),
                );
            };
            // An omitted duration leaves the expiry where it is, rather than
            // taking the default a new link would get: a change of memo is not
            // a request to start the clock again.
            let until = if wanted.until.trim().is_empty() {
                None
            } else {
                match link_expiry(&wanted.until) {
                    Ok(until) => Some(until),
                    Err(message) => return write_json(400, &json!({"error": message})),
                }
            };
            if wanted.budget.is_some_and(|budget| budget < 0) {
                return write_json(
                    400,
                    &json!({"error": "a link budget is a non-negative number of comments per hour"}),
                );
            }
            let label = wanted
                .label
                .as_deref()
                .map(|label| clean(label, MAX_LINK_LABEL).trim().to_string());
            adjusted = Some(LinkSettings {
                role,
                until,
                label,
                budget: wanted.budget,
            });
        }

        let revoke = asked.revoke.clone().unwrap_or_default();
        let now = crate::util::now_unix();
        let mutation_actor = crate::document::store::MutationActor {
            account_id: current_who.id.id.clone(),
            owner_key: current_who.key.clone(),
            session_generation: current_who.id.session_generation.clone(),
            link_hash: current_who.link.clone(),
            policy_editor: self.publishers.allows(&current_who.id.handle),
            unowned_publisher: false,
        };
        let updated = self
            .store
            .modify_as_owner(slug, &mutation_actor, |entry| {
                let asked_revoke = revoke.trim();
                if !asked_revoke.is_empty() {
                    let went =
                        parse_role_word(asked_revoke).is_some_and(|role| entry.drop_link(role));
                    if !went {
                        return Err(format!("nothing shared with {asked_revoke:?} to revoke"));
                    }
                    entry.prune_guests(now);
                }
                if let Some(link) = &minted {
                    let mut link = link.clone();
                    // CLI callers may rotate a key without repeating its
                    // memo. An explicit empty label clears it; an omitted one
                    // preserves it. A missing budget returns to the ordinary
                    // deployment limit.
                    if let Some(existing) = entry.link_for(link.granted()) {
                        if asked
                            .link
                            .as_ref()
                            .is_some_and(|wanted| wanted.label.is_none())
                        {
                            link.label.clone_from(&existing.label);
                        }
                        // The same for the expiry: a rotation asked for
                        // without a duration is a new key for the link that is
                        // there, not a link that now runs to a different day.
                        if asked
                            .link
                            .as_ref()
                            .is_some_and(|wanted| wanted.until.trim().is_empty())
                        {
                            link.until.clone_from(&existing.until);
                        }
                    }
                    entry.set_link(link);
                    // A rotated link's old hash names nothing any more, so a
                    // guest recorded against it is not a guest of this
                    // document's link any longer either.
                    entry.prune_guests(now);
                }
                if let Some(settings) = &adjusted {
                    let role = settings.role;
                    let Some(existing) = entry.link_for(role) else {
                        return Err(format!("nothing shared with {:?} to change", role.as_str()));
                    };
                    let mut link = existing.clone();
                    if let Some(until) = &settings.until {
                        link.until.clone_from(until);
                    }
                    // An omitted label keeps the memo, the way rotation does;
                    // an empty one clears it.
                    if let Some(label) = &settings.label {
                        link.label.clone_from(label);
                    }
                    link.budget = settings.budget;
                    entry.set_link(link);
                }
                Ok(())
            })
            .await;
        let entry = match updated {
            Ok(entry) => entry,
            Err(ModifyError::NotFound) => return write_json(404, &json!({"error": "not found"})),
            Err(ModifyError::Refused(message)) => {
                return write_json(400, &json!({"error": message}))
            }
            Err(ModifyError::Storage(err)) => {
                eprintln!("could not record the sharing of {slug}: {err}");
                return write_json(500, &json!({"error": "could not record the change"}));
            }
        };
        // Rotations and revocations both land here: whatever just changed,
        // any socket already open on this document may no longer be entitled
        // to what it is holding.
        self.reauthorize(slug).await;
        // The minted key needs no special handling on the way out any more:
        // it was sealed into the row that was just written, and `sharing_json`
        // says the URL of every link it can open -- this one included.
        write_json(200, &self.sharing_json(&entry))
    }

    /// Everything the share dialog draws: the owner's own link, the three
    /// role links, and what this
    /// deployment's switches will let the owner offer. Only the owner ever
    /// asks for this now, so nothing here is held back from the caller.
    pub(super) fn sharing_json(&self, entry: &IndexEntry) -> Value {
        let now = crate::util::now_unix();
        // One row per role, or null where the document has never had one.
        // `url` is built exactly the way `handle_publish` builds a document's
        // own `url`: a path on this same origin, since that is what a dialog
        // reading this response is already served from.
        let link_json = |role: Role| -> Value {
            let Some(link) = entry.link_for(role) else {
                return Value::Null;
            };
            // The key itself, from the sealed copy the catalogue keeps. Only
            // the owner is ever answered here, and this is the one thing they
            // cannot get anywhere else: a link is its URL, and a link they
            // cannot copy is one they cannot hand to anybody. Empty where the
            // seal cannot be opened, which the dialog reads as "no URL to
            // offer" and answers with the offer to replace the link.
            let key = open_link_key(&self.key, &link.sealed);
            let url = if key.is_empty() {
                String::new()
            } else {
                format!("/docs/{}#k={key}", entry.slug)
            };
            json!({
                "key": key,
                "url": url,
                "since": link.since,
                "until": link.until,
                "label": link.label,
                "budget": link.budget,
                "expired": !link.live_at(now),
            })
        };
        json!({
            "slug": entry.slug,
            // The owner's own way in: the bare URL, which opens for the owner
            // by their sign-in and for nobody else. It is a link in the
            // dialog's sense only so far as it can be copied; there is
            // nothing to mint, rotate or expire about it.
            "url": format!("/docs/{}", entry.slug),
            // A visitor's key names a browser rather than a person, and is the
            // same value that owns their other uploads, so it is not a thing
            // to print: the dialog says "this browser" instead.
            "owner": {
                "login": if entry.publisher.starts_with(VISITOR_PREFIX) {
                    String::new()
                } else {
                    entry.publisher.clone()
                },
                "name": if entry.publisher.starts_with(VISITOR_PREFIX) {
                    ""
                } else {
                    entry.owner_name()
                },
                "provider": provider_of(&entry.publisher_id),
                "id": entry.publisher_id,
                "visitor": entry.publisher.starts_with(VISITOR_PREFIX),
            },
            "links": {
                "reader": link_json(Role::Reader),
                "commenter": link_json(Role::Commenter),
                "editor": link_json(Role::Editor),
            },
            "can_share": true,
            // What the deployment's switches allow, so the dialog offers only
            // the choices that would actually be accepted.
            "publishers": self.publishers.public_description(),
            "commenters_policy": self.commenters.public_description(),
            // Whether editing, and commenting, ask for a sign-in at all: a
            // link cannot carry a role the deployment itself would refuse an
            // anonymous caller.
            "edit_needs_signin": true,
            "comment_needs_signin": !self.commenters.public,
        })
    }

    /// Hands a document to another account: its history, its comments and its
    /// quota go with it, because all three are counted against the publisher.
    /// The new owner must satisfy `--publishers`, since they are about to be
    /// the person putting this document on the server.
    pub(super) async fn handle_transfer(
        &self,
        request: Request<Body>,
        arrival: &Arrival,
        slug: &str,
    ) -> Reply {
        if cross_site_refused(request.headers(), arrival) {
            return write_json(403, &cross_site_refusal());
        }
        // Establish the credential before reading an attacker-controlled body
        // or looking up a slug.  In particular, a revoked cookie must not be
        // able to reach account lookup and then win a stale ownership check.
        match self
            .authenticated_identity(request.headers(), arrival)
            .await
        {
            Ok(identity) if self.provider_configured(&identity) => {}
            Ok(_) => {
                return write_json(
                    401,
                    &json!({"error": "authentication provider is not configured"}),
                )
            }
            Err(AuthenticationFailure::Invalid) => {
                let mut response = write_json(
                    401,
                    &json!({"error": "authentication expired or was revoked"}),
                );
                self.clear_dead_session(&mut response, request.headers(), arrival)
                    .await;
                return response;
            }
            Err(AuthenticationFailure::Unavailable) => {
                return write_json(
                    503,
                    &json!({"error": "authentication service temporarily unavailable"}),
                )
            }
        }
        if !self.valid_slug(slug) {
            return write_json(400, &json!({"error": "bad slug"}));
        }
        let entry = match self.checked_entry(slug).await {
            Ok(Some(entry)) => entry,
            Ok(None) => return write_json(404, &json!({"error": "not found"})),
            Err(response) => return response,
        };
        let headers = request.headers().clone();
        let who = self.viewer(&entry, &headers, arrival, None).await;
        if !who.at_least(Role::Owner) {
            return write_json(404, &json!({"error": "not found"}));
        }
        let Ok(body) = to_bytes(request.into_body(), 1 << 16).await else {
            return write_json(400, &json!({"error": "bad request"}));
        };
        #[derive(Deserialize, Default)]
        struct Body_ {
            #[serde(default)]
            to: String,
        }
        let asked: Body_ = serde_json::from_slice(&body).unwrap_or_default();
        if asked.to.trim().is_empty() {
            return write_json(400, &json!({"error": "name the account to transfer to"}));
        }
        if names_an_address(&asked.to) {
            return write_json(404, &json!({"error": EMAIL_GRANTS_UNAVAILABLE}));
        }
        let Some(account) = self.accounts.lookup(&asked.to).await else {
            return write_json(404, &json!({"error": no_such_account(&asked.to)}));
        };
        if !self.publishers.allows(&account.handle) {
            return write_json(
                403,
                &json!({"error": format!(
                    "{} may not publish here; this deployment's --publishers allows {}",
                    account.handle, self.publishers.public_description()
                )}),
            );
        }
        // Account lookup is asynchronous. Re-resolve the owner after it
        // returns so revocation or transfer during that wait cannot proceed.
        let current_entry = match self.checked_entry(slug).await {
            Ok(Some(entry)) => entry,
            Ok(None) => return write_json(404, &json!({"error": "not found"})),
            Err(response) => return response,
        };
        let current_who = self.viewer(&current_entry, &headers, arrival, None).await;
        if current_who.auth_failed || !current_who.at_least(Role::Owner) {
            return write_json(404, &json!({"error": "not found"}));
        }
        // The earlier viewer check is only for a non-enumerating HTTP reply.
        // Recheck ownership in the authoritative PostgreSQL transaction before
        // changing anything; transfers and revocations racing
        // this request therefore have a single winner.
        {
            let catalog = &self.store.catalog;
            let target = match catalog
                .upsert_registered_account(crate::storage::postgres::NewAccount {
                    kind: "registered".into(),
                    provider: Some(account.provider.clone()),
                    provider_subject: Some(account.id.clone()),
                    handle: account.handle.clone(),
                    display_name: account.name.clone(),
                    email: None,
                })
                .await
            {
                Ok(target) => target,
                Err(error) => {
                    eprintln!("could not record transfer target: {error}");
                    return write_json(503, &json!({"error":"catalogue temporarily unavailable"}));
                }
            };
            let Some(document) = catalog.document_by_slug(slug).await.ok().flatten() else {
                return write_json(404, &json!({"error":"not found"}));
            };
            if uuid::Uuid::parse_str(&current_who.id.id).ok() != Some(document.owner_id) {
                return write_json(409, &json!({"error":"ownership changed"}));
            }
            if let Err(error) = catalog
                .update_document_identity(document.id, &document.title, target.id, "owned")
                .await
            {
                eprintln!("could not transfer {slug}: {error}");
                return write_json(500, &json!({"error":"could not record the change"}));
            }
        }
        // The catalogue path has already committed the ownership change in
        // its authoritative transaction. Reload that row into the Store cache
        // rather than applying the legacy closure a second time: that closure
        // quite correctly sees the new owner and would reject a successful
        // transfer as an ownership race. Legacy stores have no catalogue
        // transaction, so retain the ownership guard around their write.
        let moved = {
            self.store
                .get_result(slug)
                .await
                .map_err(|error| ModifyError::Storage(error.to_string()))
                .and_then(|entry| entry.ok_or(ModifyError::NotFound))
        };
        match moved {
            Ok(entry) => {
                // The old owner is very likely still connected. Reauthorize
                // every socket against the new catalogue owner immediately.
                self.reauthorize(slug).await;
                write_json(
                    200,
                    &json!({"slug": entry.slug, "owner": entry.publisher, "title": entry.title}),
                )
            }
            Err(ModifyError::NotFound) => write_json(404, &json!({"error": "not found"})),
            Err(ModifyError::Refused(message)) => write_json(400, &json!({"error": message})),
            Err(ModifyError::Storage(err)) => {
                eprintln!("could not transfer {slug}: {err}");
                write_json(500, &json!({"error": "could not record the change"}))
            }
        }
    }
}

#[cfg(test)]
mod link_key_tests {
    use super::*;

    /// What the catalogue holds is the key, and only to the deployment that
    /// sealed it. The panel's offer to copy a link rests on this: the row
    /// carries the URL back, and a copy of the catalogue without the secrets
    /// directory carries nothing.
    #[test]
    fn a_sealed_key_comes_back_only_to_the_key_that_sealed_it() {
        let session = b"a deployment's session key".to_vec();
        let key = mint_link_key();
        let sealed = seal_link_key(&session, &key);
        assert_ne!(
            sealed,
            key.as_bytes(),
            "the key is not written down as it is"
        );
        assert!(!sealed
            .windows(key.len())
            .any(|window| window == key.as_bytes()));
        assert_eq!(open_link_key(&session, &sealed), key);
        assert_eq!(
            open_link_key(b"another deployment", &sealed),
            "",
            "another deployment's secret opens nothing"
        );
        assert_eq!(
            open_link_key(&session, &[]),
            "",
            "and a link from before the catalogue kept a key has nothing to open"
        );
    }

    /// Two seals of the same key differ, because each carries its own nonce.
    /// Nothing depends on this, but a deterministic ciphertext would leak that
    /// two documents were shared with the same key, which is not a thing this
    /// column should be able to say.
    #[test]
    fn sealing_the_same_key_twice_writes_down_two_different_things() {
        let session = b"a deployment's session key".to_vec();
        let key = mint_link_key();
        assert_ne!(seal_link_key(&session, &key), seal_link_key(&session, &key));
    }
}
