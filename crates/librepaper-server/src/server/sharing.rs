//! Who else may see a document: links that carry a role.

use super::*;

/// The header a browser presents a link key on, and the query parameter the
/// socket carries it in -- the one place a browser cannot set a header. The
/// key itself never reaches a log: what is recorded anywhere is its hash.
pub const LINK_HEADER: &str = "x-librepaper-key";

pub const LINK_PARAM: &str = "k";

/// How long a new link lasts unless another expiry is asked for. A round of
/// review has an end, and a link that lives for ever is a leak waiting for a
/// forwarded email; the dialog offers to renew, which mints a new key.
pub const LINK_DEFAULT_SECONDS: i64 = 7 * 24 * 3600;

pub(super) const MAX_LINK_LABEL: usize = 80;

/// Reads a budget that may be omitted, null or a number. Omitted is None (the
/// default), null is Some(None) and a number is Some(Some(n)), so the three
/// cases stay apart.
fn double_option<'de, D>(deserializer: D) -> Result<Option<Option<i64>>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    Option::<i64>::deserialize(deserializer).map(Some)
}

/// What one call to the share route asks for. Exactly one of the four fields
/// is present: a request that names none, or several, is refused.
#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub(super) struct ShareRequest {
    #[serde(default)]
    pub(super) create: Option<CreateRequest>,
    /// Changes to a row's name, expiry or budget, leaving its key alone.
    #[serde(default)]
    pub(super) update: Option<UpdateRequest>,
    /// A new key for a row that keeps its kind, role, name, expiry and budget.
    #[serde(default)]
    pub(super) replace: Option<ReplaceRequest>,
    #[serde(default)]
    pub(super) revoke: Option<RevokeRequest>,
}

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub(super) struct CreateRequest {
    /// `link` or `token`.
    #[serde(default)]
    pub(super) kind: String,
    #[serde(default)]
    pub(super) role: String,
    /// A memo for the owner. Required for a token, where it is the name the
    /// token's work is shown under.
    #[serde(default)]
    pub(super) name: String,
    /// A duration such as `7d` or `24h`. A link may also say `never`, and an
    /// absent one gets the default. A token needs one and refuses `never`.
    #[serde(default)]
    pub(super) until: String,
    /// Comment actions per clock hour; null or absent uses the deployment's
    /// ordinary comment limit.
    #[serde(default)]
    pub(super) budget: Option<i64>,
}

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub(super) struct UpdateRequest {
    #[serde(default)]
    pub(super) id: String,
    #[serde(default)]
    pub(super) name: Option<String>,
    /// Absent or empty leaves the expiry where it is.
    #[serde(default)]
    pub(super) until: Option<String>,
    /// Absent keeps the budget, null returns it to the deployment's ordinary
    /// comment limit, a number sets it.
    #[serde(default, deserialize_with = "double_option")]
    pub(super) budget: Option<Option<i64>>,
}

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub(super) struct ReplaceRequest {
    #[serde(default)]
    pub(super) id: String,
    /// A fresh expiry, under the rules of create. Absent keeps the row's own.
    #[serde(default)]
    pub(super) until: Option<String>,
}

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub(super) struct RevokeRequest {
    #[serde(default)]
    pub(super) id: String,
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
        librepaper_document::document::retention::parse_retention(asked)
            .map_err(|_| "an expiry is a duration such as 180d or 24h, or 'never'".to_string())?
    };
    if seconds <= 0 {
        return Err("an expiry is a duration such as 180d or 24h, or 'never'".to_string());
    }
    // A day too far to write down would come back empty, which reads as no
    // expiry at all, so it is refused rather than stored.
    librepaper_base::util::now_unix()
        .checked_add(seconds)
        .map(librepaper_base::util::format_unix)
        .filter(|until| !until.is_empty())
        .ok_or_else(|| "that expiry is too far away".to_string())
}

/// When a new token stops working. A token must expire, so an absent expiry
/// and `never` are both refused.
pub(super) fn token_expiry(asked: &str) -> Result<String, String> {
    let asked = asked.trim();
    if asked.is_empty() || asked.eq_ignore_ascii_case("never") {
        return Err("a token needs an expiry such as 1d, 7d, 30d, 180d or 365d".to_string());
    }
    link_expiry(asked)
}

/// The provider namespace of a catalog account identity.
pub(super) fn provider_of(id: &str) -> String {
    id.split_once(':')
        .map(|(provider, _)| provider.to_string())
        .unwrap_or_default()
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
    hex::encode(librepaper_base::util::random_bytes(32))
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
    let nonce = librepaper_base::util::random_bytes(24);
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
    /// Mints a read link for this document and returns the path to hand out.
    /// The key is generated before the write, as the share route does, since
    /// the write is what commits it. The document's other links stay as they
    /// are.
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
            since: librepaper_base::util::timestamp(),
            until: librepaper_base::util::format_unix(
                librepaper_base::util::now_unix() + LINK_DEFAULT_SECONDS,
            ),
            ..Default::default()
        };
        let actor = caller
            .authority(true)
            .map_err(|InvalidCredential| ModifyError::Refused("invalid account".into()))?;
        self.store
            .modify_as_owner(slug, &actor, |entry| {
                entry.add_link(link.clone());
                Ok(())
            })
            .await?;
        Ok(format!("/docs/{slug}/{key}"))
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
        // The link key arrives in its header. The query form is for the socket
        // handshake only.
        let who = self.viewer(&entry, &headers, arrival, None).await;
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
        let actions = [
            asked.create.is_some(),
            asked.update.is_some(),
            asked.replace.is_some(),
            asked.revoke.is_some(),
        ];
        if actions.iter().filter(|present| **present).count() != 1 {
            return write_json(
                400,
                &json!({"error": "a share request is exactly one of create, update, replace or revoke"}),
            );
        }
        let current_entry = match self.checked_entry(slug).await {
            Ok(Some(entry)) => entry,
            Ok(None) => return write_json(404, &json!({"error": "not found"})),
            Err(response) => return response,
        };
        let current_who = self.viewer(&current_entry, &headers, arrival, None).await;
        if current_who.auth_failed || !current_who.at_least(Role::Owner) {
            return write_json(403, &json!({"error": "ownership changed"}));
        }

        // Everything a bad request can get wrong is answered here, with the
        // reason, before the write. A new key is minted before the write too,
        // because the write is what commits it; whether it does anything for
        // anybody is `role_of`'s business from here on.
        let mut created: Option<Value> = None;
        let mut add: Option<LinkGrant> = None;
        let mut swap: Option<(String, LinkGrant)> = None;
        let mut change: Option<(String, UpdateRequest)> = None;
        let mut remove: Option<String> = None;
        if let Some(wanted) = &asked.create {
            match self.plan_create(slug, &arrival.reader_origin(), wanted, &current_who) {
                Ok((grant, shown)) => {
                    created = Some(shown);
                    add = Some(grant);
                }
                Err(message) => return write_json(400, &json!({"error": message})),
            }
        }
        if let Some(wanted) = asked.update {
            let Some(existing) = current_entry.link_by_id(&wanted.id) else {
                return write_json(404, &json!({"error": "nothing is shared under that id"}));
            };
            if let Err(message) = check_update(existing.kind, &wanted) {
                return write_json(400, &json!({"error": message}));
            }
            change = Some((wanted.id.clone(), wanted));
        }
        if let Some(wanted) = &asked.replace {
            let Some(existing) = current_entry.link_by_id(&wanted.id) else {
                return write_json(404, &json!({"error": "nothing is shared under that id"}));
            };
            match self.plan_replace(
                slug,
                &arrival.reader_origin(),
                existing,
                wanted,
                &current_who,
            ) {
                Ok((grant, shown)) => {
                    created = Some(shown);
                    swap = Some((wanted.id.clone(), grant));
                }
                Err(message) => return write_json(400, &json!({"error": message})),
            }
        }
        if let Some(wanted) = &asked.revoke {
            if current_entry.link_by_id(&wanted.id).is_none() {
                return write_json(404, &json!({"error": "nothing is shared under that id"}));
            }
            remove = Some(wanted.id.clone());
        }

        let now = librepaper_base::util::now_unix();
        let Ok(mutation_actor) =
            current_who.authority(librepaper_base::role::Ceiling {
                comment: self.commenters.allows(&current_who.id),
                edit: self.publishers.allows(&current_who.id),
            })
        else {
            return write_json(401, &json!({"error": "invalid account"}));
        };
        let updated = self
            .store
            .modify_as_owner(slug, &mutation_actor, |entry| {
                if let Some(link) = &add {
                    entry.add_link(link.clone());
                }
                if let Some((id, link)) = &swap {
                    if !entry.replace_link(id, link.clone()) {
                        return Err("nothing is shared under that id".to_string());
                    }
                    // The old key names nothing any more, so a guest recorded
                    // against it is not a guest of this document's link either.
                    entry.prune_guests(now);
                }
                if let Some((id, wanted)) = &change {
                    let Some(existing) = entry.link_by_id(id) else {
                        return Err("nothing is shared under that id".to_string());
                    };
                    let mut link = existing.clone();
                    apply_update(&mut link, wanted)?;
                    entry.replace_link(id, link);
                }
                if let Some(id) = &remove {
                    if !entry.drop_link(id) {
                        return Err("nothing is shared under that id".to_string());
                    }
                    entry.prune_guests(now);
                }
                Ok(())
            })
            .await;
        let entry = match updated {
            Ok(entry) => entry,
            Err(ModifyError::NotFound) => return write_json(404, &json!({"error": "not found"})),
            Err(ModifyError::Refused(message)) => {
                let status = if message == "ownership changed" {
                    409
                } else {
                    400
                };
                return write_json(status, &json!({"error": message}));
            }
            Err(ModifyError::Storage(err)) => {
                tracing::warn!("could not record the sharing of {slug}: {err}");
                return write_json(500, &json!({"error": "could not record the change"}));
            }
        };
        // Replacements and revocations both land here: whatever just changed,
        // any socket already open on this document may no longer be entitled
        // to what it is holding.
        self.reauthorize(slug).await;
        let mut shown = self.sharing_json(&entry);
        if let Some(created) = created {
            shown["created"] = created;
        }
        write_json(200, &shown)
    }

    /// The row a `create` asks for and the `created` object that tells the
    /// owner its credential. The credential is shown once: a link's key is
    /// also sealed into its row, a token's key is kept nowhere.
    fn plan_create(
        &self,
        slug: &str,
        origin: &str,
        wanted: &CreateRequest,
        owner: &Viewer,
    ) -> Result<(LinkGrant, Value), String> {
        if matches!(wanted.budget, Some(budget) if budget < 0) {
            return Err("a budget is a non-negative number of comments per hour".to_string());
        }
        let name = clean(&wanted.name, MAX_LINK_LABEL).trim().to_string();
        let key = mint_link_key();
        let mut grant = LinkGrant {
            hash: hash_link_key(&key),
            since: librepaper_base::util::timestamp(),
            label: name.clone(),
            budget: wanted.budget,
            ..Default::default()
        };
        let shown = match wanted.kind.trim() {
            "link" => {
                let role = wanted
                    .role
                    .parse::<Role>()
                    .map_err(|_| "a link is 'reader', 'commenter' or 'editor'")?;
                grant.role = role.as_str().to_string();
                grant.until = link_expiry(&wanted.until)?;
                grant.sealed = seal_link_key(&self.key, &key);
                json!({"id": grant.id(), "kind": "link", "url": format!("/docs/{slug}/{key}")})
            }
            "token" => {
                let role = wanted
                    .role
                    .parse::<Role>()
                    .map_err(|_| "a token is 'read', 'comment' or 'edit'")?;
                if name.is_empty() {
                    return Err("a token needs a name".to_string());
                }
                grant.role = role.as_str().to_string();
                grant.until = token_expiry(&wanted.until)?;
                grant.kind = LinkKind::Agent;
                grant.owner_generation = owner.id.session_generation.clone();
                json!({"id": grant.id(), "kind": "token", "token": agent_token_string(origin, slug, &key)})
            }
            _ => return Err("a share is a 'link' or a 'token'".to_string()),
        };
        Ok((grant, shown))
    }

    /// The row that stands in for `existing` under a new key, and its
    /// `created` object. Kind, role, name and budget carry over, and so does
    /// the expiry unless the request gives a fresh one.
    fn plan_replace(
        &self,
        slug: &str,
        origin: &str,
        existing: &LinkGrant,
        wanted: &ReplaceRequest,
        owner: &Viewer,
    ) -> Result<(LinkGrant, Value), String> {
        let key = mint_link_key();
        let mut grant = existing.clone();
        grant.hash = hash_link_key(&key);
        grant.since = librepaper_base::util::timestamp();
        if let Some(until) = wanted
            .until
            .as_deref()
            .filter(|until| !until.trim().is_empty())
        {
            grant.until = match existing.kind {
                LinkKind::Link => link_expiry(until)?,
                LinkKind::Agent => token_expiry(until)?,
            };
        }
        let shown = match existing.kind {
            LinkKind::Link => {
                grant.sealed = seal_link_key(&self.key, &key);
                json!({"id": grant.id(), "kind": "link", "url": format!("/docs/{slug}/{key}")})
            }
            LinkKind::Agent => {
                grant.sealed = Vec::new();
                grant.owner_generation = owner.id.session_generation.clone();
                json!({"id": grant.id(), "kind": "token", "token": agent_token_string(origin, slug, &key)})
            }
        };
        Ok((grant, shown))
    }

    /// Everything the Sharing panel draws: the owner's own URL, every live
    /// link and agent token, oldest first, and what this deployment's switches will let the owner offer.
    /// Only the owner ever asks for this, so nothing here is held back from
    /// the caller. A token never carries its key: nothing keeps it.
    pub(super) fn sharing_json(&self, entry: &IndexEntry) -> Value {
        let now = librepaper_base::util::now_unix();
        let mut links = Vec::new();
        let mut tokens = Vec::new();
        // An expired row admits nobody, so the panel is not shown it.
        for row in entry.links.iter().filter(|row| row.live_at(now)) {
            match row.kind {
                LinkKind::Link => {
                    // The key itself, from the sealed copy the catalogue keeps.
                    // It is the one thing the owner cannot get anywhere else: a
                    // link is its URL. Empty where the seal cannot be opened,
                    // which the panel reads as "no URL to offer" and answers
                    // with the offer to replace the link.
                    let key = open_link_key(&self.key, &row.sealed);
                    let url = if key.is_empty() {
                        String::new()
                    } else {
                        format!("/docs/{}/{key}", entry.slug)
                    };
                    links.push(json!({
                        "id": row.id(),
                        "role": row.granted().as_str(),
                        "name": row.label,
                        "url": url,
                        "key": key,
                        "since": row.since,
                        "until": row.until,
                        "budget": row.budget,
                    }));
                }
                LinkKind::Agent => tokens.push(json!({
                    "id": row.id(),
                    "role": row.role,
                    "name": row.label,
                    "since": row.since,
                    "until": row.until,
                    "budget": row.budget,
                })),
            }
        }
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
            "links": links,
            "tokens": tokens,
            "can_share": true,
            // What the deployment's switches allow, so the dialog offers only
            // the choices that would actually be accepted.
            "publishers": self.publishers.public_description(),
            "commenters_policy": self.commenters.public_description(),
        })
    }
}

/// Checks an `update` against the row it names, so a bad duration or a bad
/// token field is answered with its reason before anything is written.
fn check_update(kind: LinkKind, wanted: &UpdateRequest) -> Result<(), String> {
    apply_update(
        &mut LinkGrant {
            kind,
            ..Default::default()
        },
        wanted,
    )
}

/// Applies an `update` to a row: only the fields it sends change, and the
/// key never does.
fn apply_update(link: &mut LinkGrant, wanted: &UpdateRequest) -> Result<(), String> {
    if matches!(wanted.budget, Some(Some(budget)) if budget < 0) {
        return Err("a budget is a non-negative number of comments per hour".to_string());
    }
    if let Some(name) = &wanted.name {
        let name = clean(name, MAX_LINK_LABEL).trim().to_string();
        if name.is_empty() && link.kind == LinkKind::Agent {
            return Err("a token needs a name".to_string());
        }
        link.label = name;
    }
    if let Some(until) = wanted
        .until
        .as_deref()
        .filter(|until| !until.trim().is_empty())
    {
        link.until = match link.kind {
            LinkKind::Link => link_expiry(until)?,
            LinkKind::Agent => token_expiry(until)?,
        };
    }
    if let Some(budget) = wanted.budget {
        link.budget = budget;
    }
    Ok(())
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

#[cfg(test)]
mod link_expiry_tests {
    use super::*;

    #[test]
    fn an_unset_expiry_defaults_to_seven_days() {
        let before = librepaper_base::util::now_unix();
        let until = link_expiry("").expect("the default expiry is valid");
        let after = librepaper_base::util::now_unix();
        let until = librepaper_base::util::parse_timestamp(&until).expect("expiry is a timestamp");

        assert!((before + 7 * 24 * 3600..=after + 7 * 24 * 3600).contains(&until));
    }

    /// A duration past what a timestamp can hold would format as empty, the
    /// value that means no expiry, so a token could be made permanent by
    /// asking for a very long one. Both kinds refuse it.
    #[test]
    fn an_expiry_too_far_away_is_refused_not_stored_as_none() {
        assert!(link_expiry("1000000000d").is_err());
        assert!(token_expiry("1000000000d").is_err());
        assert!(token_expiry("180d").is_ok_and(|until| !until.is_empty()));
    }

    /// A token for a CI job lasts a year.
    #[test]
    fn a_token_may_last_a_year() {
        let before = librepaper_base::util::now_unix();
        let until = token_expiry("365d").expect("a year is a valid token expiry");
        let after = librepaper_base::util::now_unix();
        let until = librepaper_base::util::parse_timestamp(&until).expect("expiry is a timestamp");

        assert!((before + 365 * 24 * 3600..=after + 365 * 24 * 3600).contains(&until));
        assert!(token_expiry("").is_err());
        assert!(token_expiry("never").is_err());
    }
}

#[cfg(test)]
mod request_shape_tests {
    use super::*;

    #[test]
    fn a_budget_is_omitted_null_or_a_number() {
        let omitted: UpdateRequest = serde_json::from_str(r#"{"id":"a"}"#).unwrap();
        assert_eq!(omitted.budget, None);
        let cleared: UpdateRequest = serde_json::from_str(r#"{"id":"a","budget":null}"#).unwrap();
        assert_eq!(cleared.budget, Some(None));
        let set: UpdateRequest = serde_json::from_str(r#"{"id":"a","budget":5}"#).unwrap();
        assert_eq!(set.budget, Some(Some(5)));
    }

    #[test]
    fn the_old_request_shapes_are_not_requests() {
        assert!(serde_json::from_str::<ShareRequest>(r#"{"link":{"role":"reader"}}"#).is_err());
        assert!(serde_json::from_str::<ShareRequest>(r#"{"revoke":"reader"}"#).is_err());
    }

    #[test]
    fn a_replace_may_carry_a_fresh_expiry() {
        let replace: ReplaceRequest = serde_json::from_str(r#"{"id":"a","until":"7d"}"#).unwrap();
        assert_eq!(replace.until.as_deref(), Some("7d"));
        let bare: ReplaceRequest = serde_json::from_str(r#"{"id":"a"}"#).unwrap();
        assert_eq!(bare.until, None);
    }

    #[test]
    fn a_token_needs_a_real_expiry() {
        assert!(token_expiry("").is_err());
        assert!(token_expiry("never").is_err());
        assert!(token_expiry("1d").is_ok());
        assert!(token_expiry("180d").is_ok());
    }

    #[test]
    fn an_update_never_empties_a_token_name_or_makes_it_permanent() {
        let mut token = LinkGrant {
            kind: LinkKind::Agent,
            label: "assistant".into(),
            ..Default::default()
        };
        let blank = UpdateRequest {
            id: "a".into(),
            name: Some(" ".into()),
            ..Default::default()
        };
        assert!(apply_update(&mut token, &blank).is_err());
        let forever = UpdateRequest {
            id: "a".into(),
            until: Some("never".into()),
            ..Default::default()
        };
        assert!(apply_update(&mut token, &forever).is_err());
        let mut link = LinkGrant::default();
        assert!(apply_update(&mut link, &forever).is_ok());
        assert!(link.until.is_empty());
    }
}
