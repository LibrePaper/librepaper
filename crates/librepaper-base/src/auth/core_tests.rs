use super::*;

#[test]
fn credentials_cannot_cross_purposes() {
    let key = [7; 32];
    let mut who =
        Identity::from_provider("google", "42", "alice@example.org", "Alice | Example", "");
    who.session_generation = "generation".into();
    let session = sign_session(&key, &who, now_unix() + 3600);
    let device = sign_device(&key, &who, now_unix() + 3600);
    let visitor = sign_visitor(&key, "0123456789abcdef0123456789abcdef");
    assert_eq!(read_session(&key, &session), who);
    assert_eq!(read_device(&key, &device), who);
    assert_eq!(
        read_visitor(&key, &visitor),
        "0123456789abcdef0123456789abcdef"
    );
    assert!(!read_session(&key, device.strip_prefix(DEVICE_TOKEN_PREFIX).unwrap()).is_signed_in());
    assert!(!read_device(&key, &format!("{DEVICE_TOKEN_PREFIX}{session}")).is_signed_in());
    assert!(read_visitor(&key, &session).is_empty());
    assert!(read_visitor(&key, device.strip_prefix(DEVICE_TOKEN_PREFIX).unwrap()).is_empty());
    assert!(!read_session(&key, &visitor).is_signed_in());
    assert!(!read_device(&key, &format!("{DEVICE_TOKEN_PREFIX}{visitor}")).is_signed_in());
    let payload = "same payload";
    let signature = sign(&key, "figure-frame-v1", payload);
    assert!(!verifies(&key, "socket-state-v1", payload, &signature));
    assert!(!read_session(&key, &sign_session(&key, &who, now_unix())).is_signed_in());
}

#[test]
fn agent_grants_are_short_lived_document_link_scoped_credentials() {
    let key = [19; 32];
    let mut identity = Identity::from_provider("google", "42", "alice@example.org", "Alice", "");
    identity.id = uuid::Uuid::now_v7().to_string();
    identity.session_generation = "7".into();
    let grant = AgentGrant {
        identity: identity.clone(),
        slug: "paper_1".into(),
        link_hash: "ab".repeat(32),
        role: 2,
        expires_at: now_unix() + 60,
    };
    let token = sign_agent_grant(&key, &grant).expect("valid grant signs");
    assert!(token.starts_with(AGENT_GRANT_PREFIX));
    assert_eq!(read_agent_grant(&key, &token), Some(grant.clone()));
    assert!(read_agent_grant(&[20; 32], &token).is_none());

    let mut overlong = grant.clone();
    overlong.expires_at = now_unix() + AGENT_GRANT_MAX_AGE.as_secs() as i64 + 1;
    assert!(sign_agent_grant(&key, &overlong).is_none());
    let mut expired = grant.clone();
    expired.expires_at = now_unix();
    assert!(sign_agent_grant(&key, &expired).is_none());
    let mut invalid_role = grant.clone();
    invalid_role.role = 4;
    assert!(sign_agent_grant(&key, &invalid_role).is_none());
    let mut invalid_link = grant;
    invalid_link.link_hash = "not-a-digest".into();
    assert!(sign_agent_grant(&key, &invalid_link).is_none());
}

#[test]
fn established_catalog_account_uuid_survives_session_round_trip() {
    let key = [11; 32];
    let mut who = Identity::github("alice", "provider-subject");
    who.id = uuid::Uuid::now_v7().to_string();
    who.session_generation = "4".into();
    let session = sign_session(&key, &who, now_unix() + 3600);
    assert_eq!(read_session(&key, &session), who);
}

#[test]
fn pictures_travel_in_current_credentials() {
    let key = [3; 32];
    let expiry = now_unix() + 3600;
    let mut who = Identity::from_provider(
        "google",
        "42",
        "alice@example.org",
        "Alice | Example",
        "https://lh3.googleusercontent.com/a/photo=s96-c",
    );
    who.session_generation = "generation".into();
    assert_eq!(
        who.picture,
        "https://lh3.googleusercontent.com/a/photo=s96-c"
    );
    assert_eq!(who.picture_url(), who.picture);
    let session = sign_session(&key, &who, expiry);
    assert!(session.starts_with("v2."));
    assert_eq!(read_session(&key, &session), who);
    assert_eq!(read_device(&key, &sign_device(&key, &who, expiry)), who);

    // A GitHub avatar is derived from the id and never stored.
    let github = Identity::github("Alice", "583231");
    assert!(github.picture.is_empty());
    assert_eq!(
        github.picture_url(),
        "https://avatars.githubusercontent.com/u/583231?s=96&v=4"
    );
    assert!(Identity::anonymous().picture_url().is_empty());

    // Pictures that are not plain https URLs are dropped rather than carried.
    for bad in [
        "http://example.org/a.png",
        "https://example.org/a|b.png",
        "javascript:alert(1)",
        "https://example.org/<img>",
        &format!("https://example.org/{}", "x".repeat(600)),
    ] {
        assert!(
            Identity::from_provider("google", "42", "alice@example.org", "Alice", bad)
                .picture
                .is_empty(),
            "kept {bad}"
        );
    }

    // Relabelling the current payload with a retired version fails rather
    // than selecting another parser.
    let v2_payload = session
        .strip_prefix("v2.")
        .unwrap()
        .split_once('.')
        .unwrap()
        .0;
    assert!(!read_session(
        &key,
        &format!("v1.{v2_payload}.{}", sign(&key, "session-v2", v2_payload))
    )
    .is_signed_in());
}

fn account(provider: &str, handle: &str) -> Identity {
    Identity::from_provider(provider, "42", handle, handle, "")
}

#[test]
fn policy_public_description_hides_entries() {
    let policy = Policy::allowing("github", &["alice", "bob"]).with("google", &["@example.org"]);
    assert_eq!(policy.public_description(), "an allowlist of 3 entries");
    assert_eq!(policy.describe(), "github: alice, bob; google: @example.org");
    assert_eq!(
        Policy::allowing("github", &["*"]).public_description(),
        "any account of github"
    );
    assert_eq!(Policy::anonymous().public_description(), "anyone");
    assert_eq!(Policy::default().public_description(), "nobody");
    assert_eq!(Policy::default().describe(), "nobody");
    assert_eq!(Policy::anonymous().describe(), "anyone");
}

#[test]
fn policy_matches_the_provider_of_the_identity() {
    let github = Policy::allowing("github", &["alice", "0000-0002-1825-0097"]);
    assert!(github.allows(&account("github", "Alice")));
    assert!(!github.allows(&account("orcid", "0000-0002-1825-0097")));
    assert!(!github.allows(&account("google", "alice")));
    let orcid = Policy::allowing("orcid", &["0000-0002-1825-0097"]);
    assert!(orcid.allows(&account("orcid", "0000-0002-1825-0097")));
    assert!(!orcid.allows(&account("github", "0000-0002-1825-0097")));
    let star = Policy::allowing("github", &["*"]);
    assert!(star.allows(&account("github", "anyone")));
    assert!(!star.allows(&account("orcid", "0000-0002-1825-0097")));
    assert!(!star.allows(&account("google", "a@example.org")));
}

#[test]
fn policy_domain_entries_match_the_exact_domain() {
    let policy = Policy::allowing("google", &["@example.org", "bob@other.org"]);
    assert!(policy.allows(&account("google", "alice@example.org")));
    assert!(policy.allows(&account("google", "bob@other.org")));
    assert!(!policy.allows(&account("google", "alice@mail.example.org")));
    assert!(!policy.allows(&account("google", "carol@other.org")));
    assert!(!policy.allows(&account("entra", "alice@example.org")));
}

#[test]
fn policy_anonymous_and_signed_out() {
    assert!(Policy::anonymous().allows(&Identity::anonymous()));
    let policy = Policy::allowing("github", &["*"]);
    assert!(!policy.allows(&Identity::anonymous()));
    assert!(!Policy::default().allows(&account("github", "alice")));
    assert!(!Policy::default().is_configured());
    assert!(Policy::anonymous().is_configured());
    assert!(policy.is_configured());
    assert!(!Policy::allowing("github", &[]).is_configured());
}

#[test]
fn normalize_entry_checks_each_provider_shape() {
    assert_eq!(normalize_entry("github", " Alice-1 ").unwrap(), "alice-1");
    assert_eq!(normalize_entry("github", "*").unwrap(), "*");
    assert!(normalize_entry("github", "@alice").unwrap_err().contains("without @"));
    for bad in ["", "-alice", "alice-", "a_b", "a@b.org", &"a".repeat(40)] {
        assert!(normalize_entry("github", bad).is_err(), "{bad}");
    }
    assert_eq!(
        normalize_entry("orcid", "0000-0002-1825-0097").unwrap(),
        "0000-0002-1825-0097"
    );
    assert_eq!(
        normalize_entry("orcid", "0000-0002-1694-233x").unwrap(),
        "0000-0002-1694-233X"
    );
    assert!(normalize_entry("orcid", "0000-0002-1825-0098").is_err());
    assert!(normalize_entry("orcid", "alice").is_err());
    assert_eq!(normalize_entry("orcid", "*").unwrap(), "*");
    for provider in ["google", "entra", "oidc-acme"] {
        assert_eq!(normalize_entry(provider, "Bob@Example.ORG").unwrap(), "bob@example.org");
        assert_eq!(normalize_entry(provider, "@Example.org").unwrap(), "@example.org");
        assert_eq!(normalize_entry(provider, "*").unwrap(), "*");
        for bad in ["alice", "@example", "@", "a@b@c.org", "a b@c.org", "alice@", "@a@b.org"] {
            assert!(normalize_entry(provider, bad).is_err(), "{bad}");
        }
    }
}

#[test]
fn concurrent_file_key_creators_agree() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("session.key");
    let gate = std::sync::Arc::new(std::sync::Barrier::new(16));
    let threads: Vec<_> = (0..16)
        .map(|_| {
            let path = path.clone();
            let gate = gate.clone();
            std::thread::spawn(move || {
                gate.wait();
                session_key_file(&path, false).unwrap()
            })
        })
        .collect();
    let keys: Vec<_> = threads
        .into_iter()
        .map(|thread| thread.join().unwrap())
        .collect();
    assert!(keys.iter().all(|key| key == &keys[0]));
    assert_eq!(session_key_file(&path, true).unwrap(), keys[0]);
    assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }
}

#[test]
fn corrupt_missing_or_unreadable_keys_are_not_replaced() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("session.key");
    assert!(session_key_file(&path, true).is_err());
    assert!(!path.exists());
    std::fs::write(&path, "corrupt").unwrap();
    assert!(session_key_file(&path, false).is_err());
    assert_eq!(std::fs::read(&path).unwrap(), b"corrupt");
    let unreadable = dir.path().join("directory-not-file");
    std::fs::create_dir(&unreadable).unwrap();
    assert!(session_key_file(&unreadable, false).is_err());
    assert!(unreadable.is_dir());
}

#[test]
fn github_identities_keep_their_shape() {
    let who = Identity::github(" Alice ", "583231");
    assert_eq!(who.provider, "github");
    assert_eq!(who.id, "github:583231");
    assert_eq!(who.handle, "alice");
    assert_eq!(who.name, "alice");
    assert!(who.picture.is_empty());
    assert!(who.session_generation.is_empty());
    assert!(!Identity::github("alice", "").is_signed_in());
}

#[test]
fn from_provider_builds_the_id_without_touching_the_subject() {
    let who = Identity::from_provider(
        "oidc-x",
        "github:583231",
        " Alice@Example.ORG ",
        "Alice",
        "",
    );
    assert_eq!(who.provider, "oidc-x");
    assert_eq!(who.id, "oidc-x:github:583231");
    assert_eq!(who.handle, "alice@example.org");
    assert_eq!(who.name, "Alice");
    assert!(who.session_generation.is_empty());
    // A padded subject is another subject, not the same one trimmed.
    assert_eq!(
        Identity::from_provider("oidc-x", " github:583231 ", "a@example.org", "", "").id,
        "oidc-x: github:583231 "
    );
    // No subject, no id, so nobody is signed in.
    let nobody = Identity::from_provider("entra", "  ", "alice@example.org", "Alice", "");
    assert!(!nobody.is_signed_in());
}

#[test]
fn from_provider_never_shows_an_address_as_the_name() {
    let name = |given: &str| {
        Identity::from_provider("oidc-x", "1", "Alice.Smith@example.org", given, "").name
    };
    assert_eq!(name(""), "alice.smith");
    assert_eq!(name("   "), "alice.smith");
    // An address-shaped name keeps its own local part, which may differ from
    // the handle's; only an empty name falls back to the handle.
    assert_eq!(name("alice.p@example.org"), "alice.p");
    assert_eq!(name("Contact: bob@example.org"), "Contact: bob");
    assert_eq!(name("@example.org"), "alice.smith");
    assert_eq!(name("  Alice Smith  "), "Alice Smith");
    // A handle with no address in it is its own local part.
    assert_eq!(
        Identity::from_provider("github", "1", "Alice", "", "").name,
        "alice"
    );
}

#[test]
fn from_provider_cleans_and_limits_the_name() {
    let name =
        |given: &str| Identity::from_provider("oidc-x", "1", "alice@example.org", given, "").name;
    assert_eq!(name(&"a".repeat(150)), "a".repeat(100));
    assert_eq!(name(&"a".repeat(100)), "a".repeat(100));
    // The limit counts characters and never cuts one in half.
    assert_eq!(name(&"\u{e9}".repeat(150)), "\u{e9}".repeat(100));
    assert_eq!(name("Al\u{7}ice\n Smith\t"), "Alice Smith");
    // A name of nothing but control characters is no name.
    assert_eq!(name("\u{7}\u{8}"), "alice");
    // A bar is allowed: the cookie parser takes the name last.
    assert_eq!(name("Alice | Example"), "Alice | Example");
}

#[test]
fn from_provider_filters_the_picture() {
    let picture = |given: &str| {
        Identity::from_provider("oidc-x", "1", "alice@example.org", "Alice", given).picture
    };
    assert_eq!(
        picture(" https://pictures.example/a.png "),
        "https://pictures.example/a.png"
    );
    assert_eq!(picture(""), "");
    assert_eq!(picture("http://pictures.example/a.png"), "");
    assert_eq!(picture("https://pictures.example/a|b.png"), "");
    assert_eq!(
        picture(&format!("https://pictures.example/{}", "x".repeat(600))),
        ""
    );
}

#[test]
fn oidc_slugs_are_a_lowercase_letter_then_up_to_23_letters_digits_or_hyphens() {
    for good in ["a", "x", "keycloak", "a-", "a-b", "abc123", "a1-b2-c3"] {
        assert!(valid_oidc_slug(good), "{good}");
    }
    assert!(valid_oidc_slug(&format!("a{}", "b".repeat(23))));
    for bad in [
        "", "1a", "-a", "A", "aB", "a_b", "a b", "a|b", "a:b", "a.b", "\u{e9}",
    ] {
        assert!(!valid_oidc_slug(bad), "{bad}");
    }
    assert!(!valid_oidc_slug(&format!("a{}", "b".repeat(24))));
}

#[test]
fn provider_ids_are_the_three_names_and_oidc_slugs() {
    for good in [
        "github",
        "google",
        "entra",
        "oidc-x",
        "oidc-keycloak",
        "oidc-a1-b2",
        "oidc-abcdefghijklmnopqrstuvwx",
    ] {
        assert!(valid_provider_id(good), "{good}");
    }
    for bad in [
        "",
        "oidc-",
        "OIDC-x",
        "oidc-X",
        "oidc-1x",
        "oidc--x",
        "oidc-x_y",
        "oidc-abcdefghijklmnopqrstuvwxy",
        "evil|x",
        "GitHub",
        "Google",
        "microsoft",
        "oidc",
        " github",
        "github ",
        "oidc-x ",
    ] {
        assert!(!valid_provider_id(bad), "{bad:?}");
    }
}

#[test]
fn sessions_and_device_tokens_of_every_provider_id_parse() {
    let key = [5; 32];
    let expiry = now_unix() + 3600;
    for (provider, id) in [
        ("github", "github:583231"),
        ("google", "google:42"),
        ("entra", "entra:0b0e6ea5-79e6-4a53-b1f4-2d02f5f6cb49"),
        ("oidc-x", "oidc-x:github:583231"),
        ("oidc-keycloak", "oidc-keycloak:subject"),
    ] {
        let who = Identity {
            provider: provider.into(),
            id: id.into(),
            handle: "alice@example.org".into(),
            name: "Alice".into(),
            picture: String::new(),
            session_generation: "generation".into(),
        };
        assert_eq!(read_session(&key, &sign_session(&key, &who, expiry)), who);
        assert_eq!(read_device(&key, &sign_device(&key, &who, expiry)), who);
    }
}

#[test]
fn sessions_naming_no_acceptable_provider_are_anonymous() {
    let key = [5; 32];
    let expiry = now_unix() + 3600;
    let signed = |provider: &str, id: &str| {
        let who = Identity {
            provider: provider.into(),
            id: id.into(),
            handle: "alice@example.org".into(),
            name: "Alice".into(),
            picture: String::new(),
            session_generation: "generation".into(),
        };
        (
            read_session(&key, &sign_session(&key, &who, expiry)),
            read_device(&key, &sign_device(&key, &who, expiry)),
        )
    };
    for provider in [
        "oidc-",
        "OIDC-x",
        "oidc-X",
        "evil|x",
        "microsoft",
        "oidc",
        "",
    ] {
        let (session, device) = signed(provider, &format!("{provider}:42"));
        assert!(!session.is_signed_in(), "{provider:?}");
        assert!(!device.is_signed_in(), "{provider:?}");
    }
    // The id has to belong to the provider the cookie names.
    let (session, device) = signed("entra", "google:42");
    assert!(!session.is_signed_in());
    assert!(!device.is_signed_in());
}

#[test]
fn agent_grants_accept_every_provider_id_and_refuse_others() {
    let key = [21; 32];
    for provider in ["github", "google", "entra", "oidc-x"] {
        let mut identity =
            Identity::from_provider(provider, "42", "alice@example.org", "Alice", "");
        identity.session_generation = "7".into();
        let grant = AgentGrant {
            identity,
            slug: "paper_1".into(),
            link_hash: "ab".repeat(32),
            role: 2,
            expires_at: now_unix() + 60,
        };
        let token = sign_agent_grant(&key, &grant).expect("valid grant signs");
        assert_eq!(read_agent_grant(&key, &token), Some(grant), "{provider}");
    }
    for provider in ["oidc-", "OIDC-x", "evil|x", ""] {
        let mut identity =
            Identity::from_provider(provider, "42", "alice@example.org", "Alice", "");
        identity.session_generation = "7".into();
        let grant = AgentGrant {
            identity,
            slug: "paper_1".into(),
            link_hash: "ab".repeat(32),
            role: 2,
            expires_at: now_unix() + 60,
        };
        assert!(sign_agent_grant(&key, &grant).is_none(), "{provider:?}");
    }
}
