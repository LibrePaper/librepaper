//! Who else may see a document: grants to accounts, links that carry a role,
//! and handing the whole document to somebody else.

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
    /// A role word (`read`/`reader`, `comment`/`commenter`, `edit`/`editor`),
    /// or, for a legacy row, a login or the first characters of a link's id.
    #[serde(default)]
    pub(super) revoke: Option<String>,
    #[serde(default)]
    pub(super) link: Option<LinkRequest>,
}

#[derive(Deserialize, Default)]
pub(super) struct LinkRequest {
    #[serde(default)]
    pub(super) role: String,
    /// A duration such as `180d` or `24h`, `never` for a link that does not
    /// expire, or absent for the default.
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

/// Removes one grant, by the login it names or by the first characters of a
/// link's id. Returns whether anything went, so a revoke that matched nothing
/// says so rather than reporting success.
/// Which provider a stored id belongs to, read off its prefix; a bare id from
/// before providers existed is GitHub's, as `stored_id` says.
pub(super) fn provider_of(id: &str) -> String {
    stored_id(id)
        .split_once(':')
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

pub(super) fn revoke_from(entry: &mut IndexEntry, asked: &str) -> bool {
    let login = asked.trim_start_matches('@').to_lowercase();
    let before = entry.editors.len() + entry.commenters.len() + entry.links.len();
    entry
        .editors
        .retain(|grant| !grant.login.eq_ignore_ascii_case(&login));
    entry
        .commenters
        .retain(|grant| !grant.login.eq_ignore_ascii_case(&login));
    // A prefix, so the id `list` prints is enough; but not a single character,
    // which would revoke more than whoever typed it meant.
    if asked.len() >= 4 {
        entry
            .links
            .retain(|link| !link.hash.starts_with(&asked.to_lowercase()));
    }
    before != entry.editors.len() + entry.commenters.len() + entry.links.len()
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
/// from, well past the 128 the spec asks for. It is returned once, to be shown
/// once; the document keeps only its digest.
pub fn mint_link_key() -> String {
    hex::encode(crate::auth::random_bytes(32))
}

impl Server {
    /// Mints this document's read link, replacing any it had, and returns the
    /// path to hand out. The key is generated before the write, as the share
    /// route does, since the write is what commits it.
    pub(super) async fn mint_read_link(&self, slug: &str) -> Result<String, ModifyError> {
        let key = mint_link_key();
        let link = LinkGrant {
            hash: hash_link_key(&key),
            role: Role::Reader.as_str().to_string(),
            key: key.clone(),
            since: crate::util::timestamp(),
            ..Default::default()
        };
        self.store
            .modify(slug, |entry| {
                entry.set_link(link.clone());
                Ok(())
            })
            .await?;
        Ok(format!("/docs/{slug}#k={key}"))
    }

    /// The read link a document already has, as a path, when it is live and
    /// its key was kept; what a revision hands back so the command line can
    /// print something worth sending without minting anything.
    pub(super) fn read_link_of(entry: &IndexEntry) -> Value {
        let now = crate::util::now_unix();
        match entry.link_for(Role::Reader) {
            Some(link) if link.live_at(now) && !link.key.is_empty() => {
                Value::String(format!("/docs/{}#k={}", entry.slug, link.key))
            }
            _ => Value::Null,
        }
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
        let mut minted: Option<(String, LinkGrant)> = None;
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
            minted = Some((
                key.clone(),
                LinkGrant {
                    hash: hash_link_key(&key),
                    role: role.as_str().to_string(),
                    key: key.clone(),
                    since: crate::util::timestamp(),
                    until,
                    label: label.unwrap_or_default(),
                    budget: wanted.budget,
                },
            ));
        }

        let revoke = asked.revoke.clone().unwrap_or_default();
        let now = crate::util::now_unix();
        let mutation_actor = crate::document::store::MutationActor {
            account_id: current_who.id.id.clone(),
            owner_key: current_who.key.clone(),
            session_generation: current_who.id.session_generation.clone(),
            link_hash: current_who.link.clone(),
            policy_editor: self.publishers.allows(&current_who.id.handle),
            automation: current_who.automation,
            unowned_publisher: false,
        };
        let updated = self
            .store
            .modify_as_owner(slug, &mutation_actor, |entry| {
                let asked_revoke = revoke.trim();
                if !asked_revoke.is_empty() {
                    // A role word revokes that role's link; anything else is a
                    // legacy row, named by login or by the first characters of
                    // a link's id.
                    let went = match parse_role_word(asked_revoke) {
                        Some(role) => entry.drop_link(role),
                        None => revoke_from(entry, asked_revoke),
                    };
                    if !went {
                        return Err(format!("nothing shared with {asked_revoke:?} to revoke"));
                    }
                    entry.prune_guests(now);
                }
                if let Some((_, link)) = &minted {
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
                    }
                    entry.set_link(link);
                    // A rotated link's old hash names nothing any more, so a
                    // guest recorded against it is not a guest of this
                    // document's link any longer either.
                    entry.prune_guests(now);
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
        let mut answer = self.sharing_json(&entry);
        if let Some((key, _)) = minted {
            // The document keeps this link's key from here on -- see
            // `LinkGrant::key` -- but it is worth putting at the top level too,
            // since this is the response the dialog and the command line are
            // actually looking at right after asking for it.
            answer["key"] = json!(key);
        }
        write_json(200, &answer)
    }

    /// Everything the share dialog draws: the owner's own link, the three
    /// role links, the legacy people still named on it, and what this
    /// deployment's switches will let the owner offer. Only the owner ever
    /// asks for this now, so nothing here is held back from the caller.
    pub(super) fn sharing_json(&self, entry: &IndexEntry) -> Value {
        let now = crate::util::now_unix();
        let people = |grants: &Vec<Grant>| -> Vec<Value> {
            grants
                .iter()
                .map(|grant| {
                    json!({
                        "login": grant.login,
                        "name": grant.shown(),
                        "provider": provider_of(&grant.id),
                        "id": grant.id,
                        "since": grant.since,
                    })
                })
                .collect()
        };
        // One row per role, or null where the document has never had one.
        // `url` is built exactly the way `handle_publish` builds a document's
        // own `url`: a path on this same origin, since that is what a dialog
        // reading this response is already served from.
        let link_json = |role: Role| -> Value {
            let Some(link) = entry.link_for(role) else {
                return Value::Null;
            };
            let url = if link.key.is_empty() {
                String::new()
            } else {
                format!("/docs/{}#k={}", entry.slug, link.key)
            };
            json!({
                "key": link.key,
                "url": url,
                "since": link.since,
                "until": link.until,
                "label": link.label,
                "budget": link.budget,
                "expired": !link.live_at(now),
            })
        };
        let mut answer = json!({
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
            "edit_needs_signin": !self.publishers.public,
            "comment_needs_signin": !self.commenters.public,
        });
        // The people named before links existed: still honoured, still
        // revocable by login, but never grown, so this is left out entirely
        // once the last of them is gone rather than shown as two empty lists
        // forever.
        if !entry.editors.is_empty() || !entry.commenters.is_empty() {
            answer["legacy"] = json!({
                "editors": people(&entry.editors),
                "commenters": people(&entry.commenters),
            });
        }
        answer
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
            Ok(_) => {}
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
        // Recheck ownership on the authoritative row under SQLite's write
        // lock before changing anything; transfers and revocations racing
        // this request therefore have a single winner.
        if let Some(catalog) = &self.store.catalog {
            let now = crate::util::timestamp();
            if let Err(error) = crate::server::upsert_account_job(
                catalog,
                crate::storage::catalog::Account {
                    id: account.id.clone(),
                    provider: account.provider.clone(),
                    handle: account.handle.clone(),
                    name: account.name.clone(),
                    email: String::new(),
                    first_seen: now.clone(),
                    last_seen: now,
                    plan: "default".into(),
                    status: "active".into(),
                    session_generation: if cfg!(test) {
                        "test-session-generation".into()
                    } else {
                        random_token()
                    },
                    erasure_cursor: None,
                },
            )
            .await
            {
                eprintln!("could not record transfer target: {error}");
                return write_json(503, &json!({"error": "catalogue temporarily unavailable"}));
            }
            let caller_id = current_who
                .id
                .is_signed_in()
                .then_some(current_who.id.id.as_str());
            // The transfer re-checks the caller id and session generation
            // inside its own write, so a caller cancelled after dispatch
            // either transferred the document under the authority it proved
            // or did not transfer it at all; there is no partial state and
            // nothing to undo.
            let transfer_slug = slug.to_string();
            let caller_id = caller_id.map(str::to_owned);
            let caller_key = current_who.key.clone();
            let caller_generation = current_who
                .id
                .is_signed_in()
                .then(|| current_who.id.session_generation.clone());
            let new_owner = account.id.clone();
            let per_owner = self.config.storage.per_owner;
            if let Err(error) = catalog
                .execute_catalog(
                    crate::server::SERVER_JOB_BYTES + transfer_slug.len(),
                    move |catalog| {
                        catalog.transfer_ownership_authorized_with_generation(
                            &transfer_slug,
                            caller_id.as_deref(),
                            &caller_key,
                            caller_generation.as_deref(),
                            &new_owner,
                            per_owner,
                        )
                    },
                )
                .await
                .map_err(crate::storage::catalog::CatalogError::from)
            {
                return match error {
                    crate::storage::catalog::CatalogError::NotFound => {
                        write_json(404, &json!({"error": "not found"}))
                    }
                    crate::storage::catalog::CatalogError::Conflict(message) => {
                        write_json(409, &json!({"error": message}))
                    }
                    error => {
                        eprintln!("could not authorize transfer of {slug}: {error}");
                        write_json(500, &json!({"error": "could not record the change"}))
                    }
                };
            }
        }
        // The catalogue path has already committed the ownership change in
        // its authoritative transaction. Reload that row into the Store cache
        // rather than applying the legacy closure a second time: that closure
        // quite correctly sees the new owner and would reject a successful
        // transfer as an ownership race. Legacy stores have no catalogue
        // transaction, so retain the ownership guard around their write.
        let moved = if self.store.catalog.is_some() {
            self.store
                .get_result(slug)
                .await
                .map_err(|error| ModifyError::Storage(error.to_string()))
                .and_then(|entry| entry.ok_or(ModifyError::NotFound))
        } else {
            self.store
                .modify(slug, |entry| {
                    if !entry.owned_by(&current_who.key, &current_who.id.id) {
                        return Err("ownership changed".into());
                    }
                    entry.publisher = account.handle.clone();
                    entry.publisher_id = account.id.clone();
                    entry.publisher_name = account.name.clone();
                    // The new owner holds everything by owning it, so a grant
                    // to them is a row that no longer says anything.
                    entry
                        .editors
                        .retain(|grant| stored_id(&grant.id) != account.id);
                    entry
                        .commenters
                        .retain(|grant| stored_id(&grant.id) != account.id);
                    Ok(())
                })
                .await
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
