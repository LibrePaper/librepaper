use super::*;

#[test]
fn credentials_cannot_cross_purposes() {
    let key = [7; 32];
    let mut who = Identity::google("42", "alice@example.org", "Alice | Example", "");
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
fn pictures_travel_in_v2_credentials_and_v1_credentials_still_read() {
    let key = [3; 32];
    let expiry = now_unix() + 3600;
    let mut who = Identity::google(
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
            Identity::google("42", "alice@example.org", "Alice", bad)
                .picture
                .is_empty(),
            "kept {bad}"
        );
    }

    // A v1 cookie from before pictures existed is the same identity without
    // one; its five fields are read as five, so a bar in the name survives.
    let v1_payload = base64url(
        format!("google|alice@example.org|google:42|generation|Alice | Example|{expiry}")
            .as_bytes(),
    );
    let v1 = format!("v1.{v1_payload}.{}", sign(&key, "session-v1", &v1_payload));
    let read = read_session(&key, &v1);
    assert_eq!(read.name, "Alice | Example");
    assert_eq!(read.handle, "alice@example.org");
    assert!(read.picture.is_empty());
    assert!(read.picture_url().is_empty());

    // Relabelling a v1 payload as v2, or the reverse, fails the signature
    // rather than shifting the fields.
    assert!(!read_session(
        &key,
        &format!("v2.{v1_payload}.{}", sign(&key, "session-v1", &v1_payload))
    )
    .is_signed_in());
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

fn legacy_signature(key: &[u8], payload: &str) -> String {
    let mut mac = HmacSha256::new_from_slice(key).unwrap();
    mac.update(payload.as_bytes());
    base64url(&mac.finalize().into_bytes())
}

#[test]
fn legacy_visitors_keep_ownership_but_other_legacy_credentials_do_not_migrate() {
    let key = [9; 32];
    let token = "0123456789abcdef0123456789abcdef";
    let legacy = format!("{token}.{}", legacy_signature(&key, token));
    assert_eq!(read_visitor(&key, &legacy), token);
    assert_eq!(
        read_visitor(&key, &sign_visitor(&key, &read_visitor(&key, &legacy))),
        token
    );
    let payload = base64url(
        format!(
            "github|alice|github:42|generation|Alice|{}",
            now_unix() + 3600
        )
        .as_bytes(),
    );
    let old_session = format!("{payload}.{}", legacy_signature(&key, &payload));
    assert!(read_visitor(&key, &old_session).is_empty());
    assert!(!read_session(&key, &old_session).is_signed_in());
    assert!(!read_device(&key, &format!("{DEVICE_TOKEN_PREFIX}{old_session}")).is_signed_in());
}

#[test]
fn policy_public_description_hides_entries_and_anygithub_stays_restricted() {
    let policy = Policy::parse("alice@example.org,bob,@private.example.org");
    assert_eq!(policy.public_description(), "an allowlist of 3 entries");
    assert!(policy.describe().contains("alice@example.org"));
    let github = Policy::parse("anygithub");
    assert!(github.is_configured());
    assert!(github.names_a_login());
    assert!(!github.names_an_email_or_domain());
    assert!(github.allows("alice"));
    assert!(!github.allows("alice@example.org"));
    assert!(!github.allows(""));
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
fn keyring_rotation_roundtrips_and_preserves_old_keys() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("links.key");
    let old = link_sealing_key_file(&path, false).unwrap();
    let keys = vec![vec![8; 32], old];
    write_link_sealing_keyring(&path, &keys).unwrap();
    assert_eq!(link_sealing_keyring_file(&path, true).unwrap(), keys);
    assert!(write_link_sealing_keyring(&path, &[vec![0; 31]]).is_err());
    assert_eq!(link_sealing_keyring_file(&path, true).unwrap(), keys);
    assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1);
}
