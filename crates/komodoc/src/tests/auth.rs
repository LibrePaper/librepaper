use std::sync::Arc;
use std::time::Duration;

use base64::Engine;
use serde_json::{json, Value};

use super::*;
use crate::auth::{
    now_unix, read_session, sign, sign_session, Identity, Policy, TokenCache, TOKEN_CACHE_CAP,
};
use crate::config::Configuration;
use crate::storage::blob::{
    self, BlobError, BlobInfo, BlobResult, BlobStore, BlobVersion, FsStore,
};
use crate::util::first_of;

#[test]
fn policies() {
    for (value, handle, allowed) in [
        ("anyone", "", true),
        ("anyone", "someone", true),
        ("any", "", false),
        // `any` is any signed-in account on any provider, not any GitHub one.
        ("any", "someone", true),
        ("any", "someone@example.org", true),
        ("vincent", "vincent", true),
        ("vincent", "Vincent", true), // GitHub logins are case-insensitive
        ("vincent", "stranger", false),
        ("vincent", "", false),
        ("alice, bob", "bob", true),
        ("alice, bob", "carol", false),
        ("", "anyone at all", false), // unconfigured allows nobody
        // An address admits that address, on either casing.
        ("alice@example.org", "alice@example.org", true),
        ("alice@example.org", "Alice@Example.ORG", true),
        ("Alice@Example.ORG", "alice@example.org", true),
        ("alice@example.org", "bob@example.org", false),
        // A domain admits the domain exactly, and not a subdomain of it.
        ("@example.org", "alice@example.org", true),
        ("@example.org", "alice@mail.example.org", false),
        ("@example.org", "alice@notexample.org", false),
        ("@example.org", "alice", false),
        // A list of logins never admits a Google account, and a list of
        // addresses never admits a GitHub one.
        ("alice,bob", "alice@example.org", false),
        ("alice@example.org,@umontreal.ca", "alice", false),
        // The three forms mix in one list.
        ("vincent, alice@example.org, @umontreal.ca", "vincent", true),
        (
            "vincent, alice@example.org, @umontreal.ca",
            "alice@example.org",
            true,
        ),
        (
            "vincent, alice@example.org, @umontreal.ca",
            "anne@umontreal.ca",
            true,
        ),
        (
            "vincent, alice@example.org, @umontreal.ca",
            "anne@mcgill.ca",
            false,
        ),
    ] {
        assert_eq!(
            Policy::parse(value).allows(handle),
            allowed,
            "Policy::parse({value:?}).allows({handle:?})"
        );
    }
}

// The switches are asked about a whole identity everywhere it matters, so the
// handle a Google account is matched on is its verified email and the name it
// is shown under is not consulted at all.
#[test]
fn policies_match_the_handle_not_the_name() {
    let google = Identity::google("10769", "Anne.Grandchamp@UMontreal.CA", "Anne Grandchamp");
    assert_eq!(google.handle, "anne.grandchamp@umontreal.ca");
    assert_eq!(google.name, "Anne Grandchamp");
    assert_eq!(google.id, "google:10769");
    assert!(Policy::parse("@umontreal.ca").allows(&google.handle));
    assert!(Policy::parse("anne.grandchamp@umontreal.ca").allows(&google.handle));
    assert!(
        !Policy::parse("anne grandchamp").allows(&google.handle),
        "a policy matched the profile name"
    );

    // No name from Google: the local part of the address stands in, and the
    // address is still the handle.
    let unnamed = Identity::google("2", "jean@example.org", "");
    assert_eq!(unnamed.name, "jean");
    assert_eq!(unnamed.handle, "jean@example.org");
}

// A bare stored id means GitHub, and a Google sub that happens to be the same
// decimal string is a different person.
#[test]
fn stored_ids_are_qualified_as_github() {
    use crate::document::store::IndexEntry;
    let mut entry = IndexEntry {
        publisher: "vincent".into(),
        publisher_id: "583231".into(),
        ..IndexEntry::default()
    };
    assert!(
        entry.owned_by("", "github:583231"),
        "a bare stored id did not match its qualified caller"
    );
    assert!(
        !entry.owned_by("", "google:583231"),
        "a Google sub matched a GitHub id with the same number"
    );
    assert!(!entry.owned_by("", ""), "an anonymous caller owned it");

    // A newly written id is already qualified, and reads back unchanged.
    entry.publisher_id = "google:583231".into();
    assert!(entry.owned_by("", "google:583231"));
    assert!(!entry.owned_by("", "github:583231"));
}

#[test]
fn session_cookies() {
    let key = b"0123456789abcdef0123456789abcdef";
    let id = Identity::github("vincent", "42");
    assert_eq!(id.id, "github:42");
    let valid = sign_session(key, &id, now_unix() + 3600);
    assert_eq!(read_session(key, &valid), id);

    // The same round trip for a Google account, whose name is neither the
    // handle nor derivable from it, so the cookie has to carry it.
    let google = Identity::google("10769", "anne@umontreal.ca", "Anne Grandchamp");
    assert_eq!(
        read_session(key, &sign_session(key, &google, now_unix() + 3600)),
        google
    );

    // A profile name containing the field separator survives the round trip:
    // the expiry is taken off the end, so the name may hold anything.
    let barred = Identity::google("3", "x@example.org", "Jean | Tremblay");
    assert_eq!(
        read_session(key, &sign_session(key, &barred, now_unix() + 3600)).name,
        "Jean | Tremblay"
    );
    assert!(
        !read_session(key, &sign_session(key, &id, now_unix() - 3600)).is_signed_in(),
        "an expired session was accepted"
    );
    let other = b"ffffffffffffffffffffffffffffffff";
    assert!(
        !read_session(other, &valid).is_signed_in(),
        "a cookie signed with another key was accepted"
    );
    // Flipping a character of the payload must invalidate the signature.
    let tampered = format!("X{}", &valid[1..]);
    assert!(
        !read_session(key, &tampered).is_signed_in(),
        "a tampered cookie was accepted"
    );
    // The oldest cookie shape carried only login|expiry. It must not be
    // accepted as though the missing id were merely empty.
    let old_payload = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .encode(format!("vincent|{}", now_unix() + 3600));
    let old_cookie = format!("{old_payload}.{}", sign(key, &old_payload));
    assert!(
        !read_session(key, &old_cookie).is_signed_in(),
        "an old two-field cookie was accepted"
    );

    // The three-field shape is a session an earlier server wrote, which was
    // necessarily GitHub. Nothing is missing from it, only implied, so it is
    // read as a GitHub session until it expires.
    let three = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .encode(format!("vincent|42|{}", now_unix() + 3600));
    let three_cookie = format!("{three}.{}", sign(key, &three));
    let who = read_session(key, &three_cookie);
    assert_eq!(who, Identity::github("vincent", "42"));
    assert_eq!(who.provider, "github");
    assert_eq!(who.id, "github:42", "a legacy id was not qualified");
    assert_eq!(who.name, "vincent", "a legacy session lost its name");
}

#[tokio::test]
async fn comment_policy_refuses_and_attributes() {
    let server = test_server_with(
        Configuration::default(),
        Policy::parse(TEST_PUBLISHER),
        Policy::parse("any"),
        true,
    )
    .await;
    let document = publish_test_document(&server.url).await;
    let slug = text(&document, "slug");
    let path = format!("/api/documents/{slug}/comments");
    let comment = json!({"type": "comment", "exact": "hello", "body": "hi", "creator": "Impostor"});

    // A comment link's role is a ceiling on what the switch grants, not a
    // grant on its own: an anonymous caller holding one is still refused
    // for lack of a signed-in account.
    let key = comment_key(&session_as(TEST_PUBLISHER), &server.url, &slug).await;
    let (status, payload) = post_keyed("", &key, &server.url, &path, comment.clone()).await;
    assert_eq!(status, 400, "anonymous comment got {status} {payload}");
    assert_eq!(text(&payload, "message"), "sign in to comment");

    // Signed in: the name on the comment is the verified login, not the one
    // the client asked for. The switch admits this account, but writing
    // still takes the commenter link the owner minted.
    let (status, payload) =
        post_keyed(&session_as("someone"), &key, &server.url, &path, comment).await;
    assert_eq!(status, 200, "signed-in comment got {status} {payload}");
    assert_eq!(payload["comment"]["creator"], "someone");
}

// Rule F's caching half: a verified token is not re-checked against GitHub on
// every request, and neither is one that fails, though a real deployment
// trusts the failure for a much shorter time.
#[tokio::test]
async fn token_cache_caches_positive_and_negative_answers() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    let cache = TokenCache::new();
    let calls = std::sync::Arc::new(AtomicUsize::new(0));
    let good = Identity::github("vincent", "1");
    let check = |calls: std::sync::Arc<AtomicUsize>, good: Identity| {
        move |token: String| {
            calls.fetch_add(1, Ordering::SeqCst);
            async move { (token == "good-token").then_some(good) }
        }
    };

    assert_eq!(
        cache
            .verify(check(calls.clone(), good.clone()), "good-token")
            .await,
        good
    );
    assert_eq!(
        cache
            .verify(check(calls.clone(), good.clone()), "good-token")
            .await,
        good
    );
    assert_eq!(
        calls.load(Ordering::SeqCst),
        1,
        "a cached positive answer made a second call"
    );

    assert!(!cache
        .verify(check(calls.clone(), good.clone()), "bad-token")
        .await
        .is_signed_in());
    assert!(!cache
        .verify(check(calls.clone(), good.clone()), "bad-token")
        .await
        .is_signed_in());
    assert_eq!(
        calls.load(Ordering::SeqCst),
        2,
        "a cached negative answer made a third call"
    );

    // An empty token is never even asked about: there is nothing to verify.
    assert!(!cache
        .verify(check(calls.clone(), good.clone()), "")
        .await
        .is_signed_in());
    assert_eq!(calls.load(Ordering::SeqCst), 2);
}

#[test]
fn describe_policy() {
    for (value, want) in [
        ("anyone", "anyone"),
        ("any", "any signed-in account"),
        // The entries are shown as they were written: an address and a domain
        // already carry the @ that tells them from a login.
        ("vincent", "vincent"),
        ("alice,bob", "alice, bob"),
        ("alice@example.org", "alice@example.org"),
        ("@umontreal.ca", "@umontreal.ca"),
        ("alice, @umontreal.ca", "alice, @umontreal.ca"),
        ("", "nobody (unconfigured)"),
    ] {
        assert_eq!(Policy::parse(value).describe(), want);
    }
    assert!(Policy::parse("anyone").public);
    assert!(
        !Policy::parse("any").public,
        "any is not public: it still needs an account"
    );
}

#[tokio::test]
async fn auth_endpoints() {
    let server = new_test_server().await;
    // The client id is public, so the CLI can ask for it before signing in.
    let (_, payload) = get_json(&server.url, "/api/auth/config").await;
    assert_eq!(payload["client_id"], "test-client");

    let (status, payload) = get_json_as(&session_as(TEST_PUBLISHER), &server.url, "/api/me").await;
    assert!(
        status == 200
            && payload["provider"] == "github"
            && payload["handle"] == TEST_PUBLISHER
            && payload["name"] == TEST_PUBLISHER
            && payload["can_publish"] == true,
        "{payload}"
    );
    let (status, payload) = get_json(&server.url, "/api/me").await;
    assert!(
        status == 200
            && payload["provider"] == ""
            && payload["handle"] == ""
            && payload["name"] == ""
            && payload["can_publish"] == false,
        "{payload}"
    );
    assert!(
        text(&payload, "publishers").contains(TEST_PUBLISHER),
        "/api/me should say who may publish"
    );
    // The page asks what it may offer, not whether one particular provider is
    // configured.
    assert_eq!(payload["providers"], json!(["github"]), "{payload}");
}

#[tokio::test]
async fn invalid_explicit_session_is_not_downgraded_to_anonymous() {
    let server = new_test_server().await;
    let response = client()
        .get(format!("{}/api/documents/not-there", server.url))
        .header("authorization", "Bearer kmd_not-a-signed-session")
        .send()
        .await
        .unwrap();
    assert_eq!(response.status().as_u16(), 401);
}

// A Google account is named to other readers by its profile name, and keyed
// on its qualified sub. Its email is the handle the switches match, and it
// reaches nobody but its owner.
#[tokio::test]
async fn a_google_account_comments_under_its_name() {
    let server = test_server_with(
        Configuration::default(),
        Policy::parse(TEST_PUBLISHER),
        Policy::parse("@umontreal.ca"),
        true,
    )
    .await;
    let document = publish_test_document(&server.url).await;
    let slug = text(&document, "slug");
    let path = format!("/api/documents/{slug}/comments");
    let comment = json!({"type": "comment", "exact": "hello", "body": "hi", "creator": "Impostor"});
    let key = comment_key(&session_as(TEST_PUBLISHER), &server.url, &slug).await;

    let anne = google_session_as("10769", "anne@umontreal.ca", "Anne Grandchamp");
    let (status, payload) = post_keyed(&anne, &key, &server.url, &path, comment.clone()).await;
    assert_eq!(
        status, 200,
        "a domain grant refused its own domain: {payload}"
    );
    assert_eq!(payload["comment"]["creator"], "Anne Grandchamp");
    let body = serde_json::to_string(&payload).unwrap();
    assert!(
        !body.contains("anne@umontreal.ca"),
        "a Google account's email reached a comment: {body}"
    );

    // A subdomain is not the domain, and the refusal names the switch rather
    // than a provider.
    let elsewhere = google_session_as("2", "bob@mail.umontreal.ca", "Bob");
    let (status, payload) = post_keyed(&elsewhere, &key, &server.url, &path, comment).await;
    assert_eq!(status, 400, "a subdomain was admitted: {payload}");
    assert_eq!(
        text(&payload, "message"),
        "bob@mail.umontreal.ca may not comment here; this deployment allows @umontreal.ca"
    );
}

// The two accounts a domain policy and a login policy each admit are disjoint,
// and ownership follows the id rather than the handle.
#[tokio::test]
async fn a_google_account_owns_what_it_published() {
    let server = test_server_with(
        Configuration::default(),
        Policy::parse("@umontreal.ca"),
        Policy::parse("anyone"),
        true,
    )
    .await;
    let anne = google_session_as("10769", "anne@umontreal.ca", "Anne Grandchamp");
    let (status, payload) = post_as(
        &anne,
        &server.url,
        "/api/documents",
        json!({"title": "A paper", "source": "hello", "source_format": "markdown"}),
    )
    .await;
    assert_eq!(status, 201, "a Google publisher was refused: {payload}");
    let slug = text(&payload, "slug");
    let entry = server
        .instance
        .store
        .get(&slug)
        .await
        .expect("the document");
    assert_eq!(entry.publisher_id, "google:10769");
    assert!(entry.owned_by("", "google:10769"));
    assert!(
        !entry.owned_by("", "github:10769"),
        "a GitHub id with the same number owned a Google account's document"
    );

    // A GitHub login is not on this deployment's list at all.
    let (status, payload) = post_as(
        &session_as("vincent"),
        &server.url,
        "/api/documents",
        json!({"title": "Another", "source": "hello", "source_format": "markdown"}),
    )
    .await;
    assert_eq!(
        status, 403,
        "a login-less policy admitted a login: {payload}"
    );
    assert_eq!(
        text(&payload, "error"),
        "vincent may not publish here; this deployment allows @umontreal.ca"
    );
}

// Startup: one refusal and two warnings, each of which is about what a policy
// asks for against what the deployment actually configured.
#[test]
fn startup_checks_the_providers_against_the_policies() {
    use crate::server::serve::sign_in_advice;
    let advice = |github, google, publishers, commenters| {
        sign_in_advice(
            github,
            google,
            &Policy::parse(publishers),
            &Policy::parse(commenters),
            ":8080",
            8080,
        )
    };

    // Nothing configured, and a policy that needs somebody signed in: the
    // message lists both ways to get a provider.
    let fatal = advice(false, false, "vincent", "anyone")
        .fatal
        .expect("a server that can sign nobody in should not start");
    assert!(
        fatal.contains("GITHUB_CLIENT_ID") && fatal.contains("GOOGLE_CLIENT_ID"),
        "{fatal}"
    );

    // Either one on its own is enough.
    assert!(advice(true, false, "vincent", "anyone").fatal.is_none());
    assert!(advice(false, true, "@example.org", "anyone")
        .fatal
        .is_none());
    // And a wholly public deployment needs neither.
    assert!(advice(false, false, "anyone", "anyone").fatal.is_none());

    // A login named with no GitHub app: one warning, and it starts.
    let warnings = advice(false, true, "vincent", "anyone").warnings;
    assert_eq!(warnings.len(), 1, "{warnings:?}");
    assert!(warnings[0].contains("GitHub login"), "{warnings:?}");

    // An address or a domain named with no Google client: the other warning.
    let warnings = advice(true, false, "alice@example.org", "anyone").warnings;
    assert_eq!(warnings.len(), 1, "{warnings:?}");
    assert!(
        warnings[0].contains("email address or a domain"),
        "{warnings:?}"
    );
    let warnings = advice(true, false, "vincent", "@example.org").warnings;
    assert_eq!(warnings.len(), 1, "{warnings:?}");
    assert!(
        warnings[0].contains("email address or a domain"),
        "{warnings:?}"
    );

    // A list that names both kinds on a deployment with both providers is
    // quiet, and so is `any`, which names nobody in particular.
    assert!(advice(true, true, "vincent, @example.org", "anyone")
        .warnings
        .is_empty());
    assert!(advice(true, false, "any", "anyone").warnings.is_empty());
}

/* --------------------------------------------------- signing in in a browser */

/// A server with both providers, the Google half pointed at a stand-in that
/// answers with `user` and insists on `verifier`.
async fn server_with_google(user: Value, verifier: &str) -> (TestServer, GoogleStandIn) {
    let google = google_stand_in(user, verifier).await;
    let server = test_server_google(
        Policy::parse("@example.org"),
        Policy::parse("anyone"),
        true,
        &google,
    )
    .await;
    // The stand-in is returned alongside, because it stops answering the
    // moment it is dropped.
    (server, google)
}

/// Signs in through the stand-in and returns the callback's response.
async fn google_callback(base: &str, state: &str, verifier: &str, next: &str) -> reqwest::Response {
    client()
        .get(format!(
            "{base}/auth/callback/google?code=a-code&state={state}"
        ))
        .header(
            "cookie",
            format!("{}={state}|{next}|{verifier}", crate::auth::STATE_COOKIE),
        )
        .send()
        .await
        .expect("the callback answers")
}

fn cookie_of(response: &reqwest::Response, name: &str) -> String {
    response
        .headers()
        .get_all("set-cookie")
        .iter()
        .filter_map(|v| v.to_str().ok())
        .find_map(|c| {
            c.strip_prefix(&format!("{name}="))
                .map(|rest| rest.split(';').next().unwrap_or_default().to_string())
        })
        .unwrap_or_default()
}

fn verified(name: &str) -> Value {
    json!({"sub": "10769", "email": "Anne@Example.org", "email_verified": true, "name": name})
}

// The door: one provider is a redirect into it, two are a choice, none is the
// 404 it has always been.
#[tokio::test]
async fn the_sign_in_door_offers_what_is_configured() {
    // GitHub only, which is every deployment that exists today.
    let github_only = new_test_server().await;
    let response = client()
        .get(format!("{}/auth/login?next=/docs/abc", github_only.url))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status().as_u16(), 302);
    assert_eq!(
        response.headers()["location"],
        "/auth/login/github?next=%2Fdocs%2Fabc"
    );

    // No provider at all: nothing to sign in to, and the page says so.
    let public = test_server_with(
        Configuration::default(),
        Policy::parse("anyone"),
        Policy::parse("anyone"),
        false,
    )
    .await;
    let response = client()
        .get(format!("{}/auth/login", public.url))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status().as_u16(), 404);

    // Both: the choice, served from the shell like the 404 page.
    let (both, _google) = server_with_google(verified("Anne"), "v").await;
    let response = client()
        .get(format!("{}/auth/login?next=/docs/abc", both.url))
        .header("accept", "text/html")
        .send()
        .await
        .unwrap();
    assert_eq!(response.status().as_u16(), 200);
    assert!(response.headers()["content-type"]
        .to_str()
        .unwrap()
        .starts_with("text/html"));
    let (_, me) = get_json(&both.url, "/api/me").await;
    assert_eq!(me["providers"], json!(["github", "google"]));
}

// The redirect into Google carries everything the callback will check: the
// state, the challenge for the verifier that rides in the cookie, and the
// prompt that lets somebody pick between two Google accounts.
#[tokio::test]
async fn the_google_redirect_carries_state_and_a_challenge() {
    let (server, _google) = server_with_google(verified("Anne"), "v").await;
    let response = client()
        .get(format!("{}/auth/login/google?next=/docs/abc", server.url))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status().as_u16(), 302);
    let target = response.headers()["location"].to_str().unwrap().to_string();
    let sent: std::collections::HashMap<String, String> = url::Url::parse(&target)
        .unwrap()
        .query_pairs()
        .into_owned()
        .collect();
    assert_eq!(sent["response_type"], "code");
    assert_eq!(sent["scope"], "openid email profile");
    assert_eq!(sent["code_challenge_method"], "S256");
    assert_eq!(sent["prompt"], "select_account");
    assert!(sent["redirect_uri"].ends_with("/auth/callback/google"));

    // The verifier is in the cookie, and the challenge is its digest, so the
    // server keeps nothing at all between the two halves of the flow.
    let state_cookie = cookie_of(&response, crate::auth::STATE_COOKIE);
    let mut fields = state_cookie.splitn(3, '|');
    let state = fields.next().unwrap();
    assert_eq!(fields.next().unwrap(), "%2Fdocs%2Fabc");
    let verifier = fields.next().expect("no verifier in the state cookie");
    assert_eq!(sent["state"], state);
    assert_eq!(
        sent["code_challenge"],
        crate::auth::pkce_challenge(verifier)
    );
}

// The callback, against the stand-in: a qualified id, the lowercased email as
// the handle, the profile name as the name, and the next path honoured.
#[tokio::test]
async fn the_google_callback_signs_in() {
    let (server, _google) = server_with_google(verified("Anne Grandchamp"), "the-verifier").await;
    let response = google_callback(&server.url, "st", "the-verifier", "%2Fdocs%2Fabc").await;
    assert_eq!(response.status().as_u16(), 302);
    assert_eq!(response.headers()["location"], "/docs/abc");
    let session = cookie_of(&response, crate::auth::SESSION_COOKIE);
    let who = crate::auth::read_session(TEST_KEY, &session);
    assert_eq!(who.provider, "google");
    assert_eq!(who.id, "google:10769");
    assert_eq!(who.handle, "anne@example.org");
    assert_eq!(who.name, "Anne Grandchamp");
    let entries = server.instance.store.list().await;
    assert_eq!(
        entries.len(),
        4,
        "first OAuth sign-in provisions four examples"
    );
    assert!(entries
        .iter()
        .all(|entry| entry.publisher_id == who.id && !entry.example));
    let again = google_callback(&server.url, "st", "the-verifier", "").await;
    assert_eq!(again.status().as_u16(), 302);
    assert_eq!(server.instance.store.list().await.len(), 4);

    // No name from Google: the address's local part stands in.
    let user = json!({"sub": "2", "email": "jean@example.org", "email_verified": true});
    let (server, _google) = server_with_google(user, "v").await;
    let response = google_callback(&server.url, "st", "v", "").await;
    let session = cookie_of(&response, crate::auth::SESSION_COOKIE);
    assert_eq!(crate::auth::read_session(TEST_KEY, &session).name, "jean");
}

// Everything the callback refuses. Each of these leaves the browser signed out.
#[tokio::test]
async fn the_google_callback_refuses() {
    // An account with no verified address has no handle, so no policy could
    // ever admit it and it is refused with a page that says why.
    let unverified =
        json!({"sub": "3", "email": "anne@example.org", "email_verified": false, "name": "Anne"});
    let (server, _google) = server_with_google(unverified, "v").await;
    let response = google_callback(&server.url, "st", "v", "").await;
    assert_eq!(response.status().as_u16(), 403);
    assert!(response.text().await.unwrap().contains("verified email"));

    let (server, _google) = server_with_google(verified("Anne"), "the-verifier").await;

    // A state that does not match the cookie is a link somebody else crafted.
    let response = client()
        .get(format!(
            "{}/auth/callback/google?code=a-code&state=other",
            server.url
        ))
        .header(
            "cookie",
            format!("{}=st||the-verifier", crate::auth::STATE_COOKIE),
        )
        .send()
        .await
        .unwrap();
    assert_eq!(response.status().as_u16(), 400);

    // No state cookie at all.
    let response = client()
        .get(format!(
            "{}/auth/callback/google?code=a-code&state=st",
            server.url
        ))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status().as_u16(), 400);

    // A cookie with no verifier in it did not start this flow.
    let response = client()
        .get(format!(
            "{}/auth/callback/google?code=a-code&state=st",
            server.url
        ))
        .header("cookie", format!("{}=st|", crate::auth::STATE_COOKIE))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status().as_u16(), 400);

    // A verifier that does not match the challenge is refused by the exchange
    // itself, which is the whole point of PKCE.
    let response = google_callback(&server.url, "st", "another-verifier", "").await;
    assert_eq!(response.status().as_u16(), 400);
    assert!(response.text().await.unwrap().contains("google refused"));
}

// An off-origin `next` is refused exactly as GitHub's is: the redirect goes
// home rather than to whatever host the query named.
#[tokio::test]
async fn the_google_callback_refuses_an_off_origin_next() {
    let (server, _google) = server_with_google(verified("Anne"), "v").await;
    let away =
        url::form_urlencoded::byte_serialize(b"https://example.com/steal").collect::<String>();
    let response = google_callback(&server.url, "st", "v", &away).await;
    assert_eq!(response.status().as_u16(), 302);
    assert_eq!(response.headers()["location"], "/");
}

#[tokio::test]
async fn signed_in_comments_are_named_by_the_account() {
    // Commenting is open to anyone here, so a name may be typed, but it is
    // never trusted: the account overrides it when there is one, and a
    // caller with no account and no visitor cookie at all gets "Anonymous"
    // rather than whatever the request claimed.
    let server = new_test_server().await;
    let document = publish_test_document(&server.url).await;
    let slug = text(&document, "slug");
    let path = format!("/api/documents/{slug}/comments");
    let comment = json!({"type": "comment", "exact": "hello", "body": "hi", "creator": "Impostor"});
    let key = comment_key(&session_as(TEST_PUBLISHER), &server.url, &slug).await;

    let (status, payload) = post_keyed(
        &session_as("someone"),
        &key,
        &server.url,
        &path,
        comment.clone(),
    )
    .await;
    assert_eq!(status, 200);
    assert_eq!(payload["comment"]["creator"], "someone");

    // No visitor cookie, and no account, is "Anonymous" -- the request's own
    // "creator" field is never taken at its word. This deployment's switch is
    // `anyone`, so the commenter link is what lets an anonymous caller in;
    // the switch alone would not have.
    let (status, payload) = post_keyed("", &key, &server.url, &path, comment).await;
    assert_eq!(status, 200);
    assert_eq!(payload["comment"]["creator"], "Anonymous");
}

#[tokio::test]
async fn annotation_kinds() {
    let server = new_test_server().await;
    let slug = text(&publish_test_document(&server.url).await, "slug");
    let path = format!("/api/documents/{slug}/comments");

    // A highlight is the passage itself, so it needs no words.
    let (status, payload) = post(
        &server.url,
        &path,
        json!({"type": "comment", "exact": "hello", "motivation": "highlighting", "body": ""}),
    )
    .await;
    assert_eq!(status, 200, "a bodyless highlight was refused: {payload}");
    assert_eq!(payload["comment"]["motivation"], "highlighting");

    // Every other kind still needs something said.
    let (status, payload) = post(
        &server.url,
        &path,
        json!({"type": "comment", "exact": "hello", "motivation": "commenting", "body": ""}),
    )
    .await;
    assert_eq!(status, 400);
    assert_eq!(text(&payload, "message"), "comment body is required");

    // A motivation this deployment does not know is stored as the default
    // rather than refused.
    let (status, payload) = post(
        &server.url,
        &path,
        json!({"type": "comment", "exact": "hello", "motivation": "musing", "body": "a remark"}),
    )
    .await;
    assert_eq!(status, 200);
    assert_eq!(payload["comment"]["motivation"], "commenting");
    assert!(payload["comment"].get("replacement").is_none());
}

#[tokio::test]
async fn region_annotations() {
    let server = new_test_server().await;
    let slug = text(&publish_test_document(&server.url).await, "slug");
    let path = format!("/api/documents/{slug}/comments");

    // A rectangle on a figure anchors an annotation, with no quotation at all.
    let (status, payload) = post(
        &server.url,
        &path,
        json!({"type": "comment", "exact": "", "body": "the axis is unlabelled",
            "region": {"image_digest": "abc123", "image_index": 1, "x": 10.5, "y": 20, "w": 30, "h": 25}}),
    )
    .await;
    assert_eq!(status, 200, "region annotation got {status} {payload}");
    let stored = &payload["comment"]["region"];
    assert!(
        stored["image_digest"] == "abc123" && stored["image_index"] == 1 && stored["x"] == 10.5,
        "{stored}"
    );

    // Neither words nor a figure is nothing to point at.
    let (status, payload) = post(
        &server.url,
        &path,
        json!({"type": "comment", "exact": "", "body": "about what?"}),
    )
    .await;
    assert_eq!(status, 400);
    assert_eq!(
        text(&payload, "message"),
        "select some text or part of a figure to comment on"
    );

    // A rectangle outside the image, or too small to see, is not one.
    for (name, spot) in [
        (
            "off the image",
            json!({"image_index": 0, "x": 90, "y": 10, "w": 30, "h": 10}),
        ),
        (
            "negative",
            json!({"image_index": 0, "x": -5, "y": 10, "w": 10, "h": 10}),
        ),
        (
            "a click",
            json!({"image_index": 0, "x": 10, "y": 10, "w": 0.1, "h": 0.1}),
        ),
        (
            "no image",
            json!({"image_index": -1, "x": 10, "y": 10, "w": 10, "h": 10}),
        ),
    ] {
        let (status, payload) = post(
            &server.url,
            &path,
            json!({"type": "comment", "exact": "", "body": "x", "region": spot}),
        )
        .await;
        assert_eq!(status, 400, "{name}: got {status} {payload}");
    }
}

#[test]
fn settings_may_be_quoted() {
    // A .env read by make keeps the quotes a shell would strip, and a client id
    // wearing quotation marks is one GitHub has never heard of.
    for (value, want) in [
        ("\"Ov23li\"", "Ov23li"),
        ("'Ov23li'", "Ov23li"),
        ("  Ov23li ", "Ov23li"),
        ("Ov23li", "Ov23li"),
        ("\"", "\""),
        ("", ""),
    ] {
        assert_eq!(first_of(&[value]), want, "first_of({value:?})");
    }
    // The first value that is not empty still wins.
    assert_eq!(first_of(&["", "\"second\"", "third"]), "second");
}

// The share dialog is read by everyone named on a document, and a Google
// account's handle is its email address, which the spec shows to nobody. So
// the dialog draws names and providers, and the handle reaches the owner
// alone -- who typed it, and names it again to revoke.
#[tokio::test]
async fn a_google_owner_is_shown_by_name_and_never_by_email() {
    let server = test_server_with(
        Configuration::default(),
        Policy::parse("@umontreal.ca, vincent"),
        Policy::parse("anyone"),
        true,
    )
    .await;
    let anne = google_session_as("10769", "anne@umontreal.ca", "Anne Grandchamp");
    let (status, payload) = post_as(
        &anne,
        &server.url,
        "/api/documents",
        json!({"title": "A paper", "source": "hello", "source_format": "markdown"}),
    )
    .await;
    assert_eq!(status, 201, "{payload}");
    let slug = text(&payload, "slug");
    let share = format!("/api/documents/{slug}/share");

    // A legacy grant, the only way a document names anybody by hand any more
    // -- the share route no longer makes one, but it still honours one
    // already on record and still lets the owner revoke it by login.
    server
        .instance
        .store
        .modify(&slug, |entry| {
            entry.editors.push(crate::document::store::Grant {
                id: "github:vincent".into(),
                login: "vincent".into(),
                since: crate::util::timestamp(),
                name: "vincent".into(),
            });
            Ok(())
        })
        .await
        .expect("the grant is recorded");

    // The share route is the owner's alone now: a named editor -- even one
    // who may still write the document through the socket -- learns nothing
    // from it, not the owner's email or anything else.
    let (status, payload) = get_json_as(&session_as("vincent"), &server.url, &share).await;
    assert_eq!(status, 404, "an editor saw the sharing: {payload}");

    // The owner sees their own login, which for a Google account is the
    // email address they signed in with, and the legacy editor's name and
    // provider.
    let (status, payload) = get_json_as(&anne, &server.url, &share).await;
    assert_eq!(status, 200, "{payload}");
    assert_eq!(text(&payload["owner"], "login"), "anne@umontreal.ca");
    assert_eq!(text(&payload["owner"], "name"), "Anne Grandchamp");
    assert_eq!(text(&payload["owner"], "provider"), "google");
    assert_eq!(text(&payload["legacy"]["editors"][0], "login"), "vincent");
    assert_eq!(text(&payload["legacy"]["editors"][0], "name"), "vincent");
    assert_eq!(text(&payload["legacy"]["editors"][0], "provider"), "github");

    let (status, payload) = post_as(&anne, &server.url, &share, json!({"revoke": "vincent"})).await;
    assert_eq!(status, 200, "{payload}");
    assert!(
        payload.get("legacy").is_none(),
        "a fully revoked legacy grant should leave no trace: {payload}"
    );

    // An entry from before names were recorded shows its login, which is a
    // GitHub login and so is its name.
    let entry = server
        .instance
        .store
        .get(&slug)
        .await
        .expect("the document");
    assert_eq!(entry.publisher_name, "Anne Grandchamp");
    let legacy = crate::document::store::IndexEntry {
        publisher: "alice".into(),
        ..Default::default()
    };
    assert_eq!(legacy.owner_name(), "alice");
}

// Google does not issue addresses with a bar in them, but the session
// cookie's payload is bar-separated and the address is a field of it, so an
// account that somehow had one is refused rather than signed in as a cookie
// whose fields had shifted.
#[tokio::test]
async fn the_google_callback_refuses_an_address_that_would_break_the_cookie() {
    let odd = json!({"sub": "4", "email": "a|b@example.org", "email_verified": true, "name": "A"});
    let (server, _google) = server_with_google(odd, "v").await;
    let response = google_callback(&server.url, "st", "v", "").await;
    assert_eq!(response.status().as_u16(), 403);
}

/// R04: a store whose `get` fails must never be treated as "there is no key
/// yet". `session_key` must fail rather than mint and persist a replacement,
/// and the originally stored key must survive untouched underneath.
#[tokio::test]
async fn transient_read_failure_does_not_rotate_key() {
    let dir = tempfile::tempdir().unwrap();
    let inner: Arc<dyn BlobStore> = Arc::new(FsStore::new(dir.path()));
    let original = crate::auth::session_key(inner.as_ref()).await.unwrap();

    struct FailRead(Arc<dyn BlobStore>);
    #[async_trait::async_trait]
    impl BlobStore for FailRead {
        async fn get(&self, _: &str) -> BlobResult<Vec<u8>> {
            Err(BlobError::Other("transient GET failure".into()))
        }
        async fn get_versioned(&self, k: &str) -> BlobResult<(Vec<u8>, BlobVersion)> {
            self.0.get_versioned(k).await
        }
        async fn put(&self, k: &str, b: Vec<u8>, t: &str) -> BlobResult<()> {
            self.0.put(k, b, t).await
        }
        async fn swap(&self, k: &str, b: Vec<u8>, e: &str) -> BlobResult<BlobVersion> {
            self.0.swap(k, b, e).await
        }
        async fn list(&self, k: &str) -> BlobResult<Vec<BlobInfo>> {
            self.0.list(k).await
        }
        async fn delete(&self, k: &[String]) -> BlobResult<()> {
            self.0.delete(k).await
        }
        fn describe(&self) -> String {
            self.0.describe()
        }
    }

    assert!(
        crate::auth::session_key(&FailRead(inner.clone()))
            .await
            .is_err(),
        "a transient read failure must fail startup, not rotate the key"
    );
    assert_eq!(
        crate::auth::session_key(inner.as_ref()).await.unwrap(),
        original,
        "the original key must be untouched after the failed read"
    );
}

/// R04: an existing key that does not parse as 32 bytes of hex is a corrupt
/// deployment, not an absent one. `session_key` must refuse it rather than
/// silently overwrite it with a fresh replacement.
#[tokio::test]
async fn malformed_session_key_is_not_overwritten() {
    let dir = tempfile::tempdir().unwrap();
    let inner: Arc<dyn BlobStore> = Arc::new(FsStore::new(dir.path()));
    inner
        .put(
            blob::SESSION_KEY_KEY,
            b"not valid hex at all".to_vec(),
            "text/plain",
        )
        .await
        .unwrap();

    assert!(
        crate::auth::session_key(inner.as_ref()).await.is_err(),
        "a malformed stored key must be reported as an error"
    );
    assert_eq!(
        inner.get(blob::SESSION_KEY_KEY).await.unwrap(),
        b"not valid hex at all".to_vec(),
        "a malformed key must never be overwritten"
    );
}

/// R04: two servers racing to initialize the same empty storage must agree on
/// one key rather than each minting and persisting their own.
#[tokio::test]
async fn concurrent_session_key_initialization_agrees() {
    let dir = tempfile::tempdir().unwrap();
    let inner: Arc<dyn BlobStore> = Arc::new(FsStore::new(dir.path()));

    let mut tasks = Vec::new();
    for _ in 0..8 {
        let store = inner.clone();
        tasks.push(tokio::spawn(async move {
            crate::auth::session_key(store.as_ref()).await.unwrap()
        }));
    }
    let mut keys = Vec::new();
    for task in tasks {
        keys.push(task.await.unwrap());
    }
    let first = keys[0].clone();
    assert!(
        keys.iter().all(|key| *key == first),
        "concurrent first-time initializations disagreed on the signing key"
    );
}

/// R33: a stream of distinct, never-repeated invalid bearer tokens must not
/// grow the cache without bound.
#[tokio::test]
async fn token_cache_caps_size_under_distinct_invalid_tokens() {
    // A long TTL so nothing expires mid-loop; this test is about the cap, not
    // the sweep.
    let cache = TokenCache::for_test(Duration::from_secs(600), Duration::from_secs(600));
    for i in 0..(TOKEN_CACHE_CAP + 500) {
        let token = format!("bad-token-{i}");
        cache.verify(|_| async { None }, &token).await;
        assert!(
            cache.len() <= TOKEN_CACHE_CAP,
            "cache grew past its cap after {i} distinct invalid tokens"
        );
    }
    assert_eq!(cache.len(), TOKEN_CACHE_CAP);
}

/// R33: expired entries are not just excluded from capacity math -- they are
/// actually removed from the map once their TTL has passed.
#[tokio::test]
async fn token_cache_sweeps_expired_entries_on_insert() {
    let cache = TokenCache::for_test(Duration::from_secs(600), Duration::from_millis(20));
    for i in 0..50 {
        cache
            .verify(|_| async { None }, &format!("stale-{i}"))
            .await;
    }
    assert_eq!(cache.len(), 50);

    tokio::time::sleep(Duration::from_millis(60)).await;
    // The insert that follows the wait is what should sweep everything that
    // expired while the cache sat idle.
    cache.verify(|_| async { None }, "fresh").await;
    assert_eq!(
        cache.len(),
        1,
        "expired entries were not swept on the next insert"
    );
}

/// A session cookie signed by a key this deployment does not hold -- what a
/// browser carries into a reseeded deployment, or one whose secrets were
/// rotated -- is not silently anonymous on the document APIs, but it is not
/// left in the browser to fail forever either. `/api/me` and the 401 both
/// clear it, so the page's next request is plainly anonymous and an
/// `anyone` deployment's front page lists its documents again.
#[tokio::test]
async fn a_dead_session_cookie_is_cleared_and_the_listing_recovers() {
    let server = test_server_with(
        Configuration::default(),
        Policy::parse("anyone"),
        Policy::parse("anyone"),
        true,
    )
    .await;
    let other_key = b"ffffffffffffffffffffffffffffffff";
    let stale = sign_session(
        other_key,
        &Identity::github("vincent", "42"),
        now_unix() + 3600,
    );
    let cookie = format!("komodoc_session={stale}");
    let clears_session = |response: &reqwest::Response| {
        response
            .headers()
            .get_all("set-cookie")
            .iter()
            .filter_map(|v| v.to_str().ok())
            .any(|c| c.starts_with("komodoc_session=;") && c.contains("Max-Age=0"))
    };

    let me = client()
        .get(format!("{}/api/me", server.url))
        .header("X-Komodoc-Client", "shell")
        .header("Cookie", &cookie)
        .send()
        .await
        .unwrap();
    assert_eq!(me.status(), 200);
    assert!(clears_session(&me), "/api/me left the dead cookie in place");
    assert_eq!(me.json::<Value>().await.unwrap()["handle"], "");

    let listing = client()
        .post(format!("{}/api/list", server.url))
        .header("X-Komodoc-Client", "shell")
        .header("Cookie", &cookie)
        .send()
        .await
        .unwrap();
    assert_eq!(listing.status(), 401);
    assert!(
        clears_session(&listing),
        "the 401 left the dead cookie in place"
    );

    // A live session is never cleared by the same path.
    let live = sign_session(
        &server.instance.key,
        &Identity::github("vincent", "42"),
        now_unix() + 3600,
    );
    let me = client()
        .get(format!("{}/api/me", server.url))
        .header("X-Komodoc-Client", "shell")
        .header("Cookie", format!("komodoc_session={live}"))
        .send()
        .await
        .unwrap();
    assert_eq!(me.status(), 200);
    assert!(!clears_session(&me), "a live session was cleared");

    // Without the cookie, which is what the browser sends next, the listing
    // is the anonymous publisher's to read.
    let listing = client()
        .post(format!("{}/api/list", server.url))
        .header("X-Komodoc-Client", "shell")
        .send()
        .await
        .unwrap();
    assert_eq!(listing.status(), 200);
}
