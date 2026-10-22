use super::*;

fn grant(hash_digit: char, role: &str, kind: LinkKind) -> LinkGrant {
    LinkGrant {
        hash: hash_digit.to_string().repeat(64),
        role: role.into(),
        kind,
        ..LinkGrant::default()
    }
}

#[test]
fn a_row_id_is_the_first_sixteen_hex_characters_of_its_digest() {
    let link = grant('a', "reader", LinkKind::Link);
    assert_eq!(link.id(), "a".repeat(16));
}

#[test]
fn agent_rows_are_invisible_to_link_lookups() {
    let mut entry = IndexEntry::default();
    entry.add_link(grant('a', "commenter", LinkKind::Agent));
    entry.add_link(grant('b', "commenter", LinkKind::Link));
    let agent = "a".repeat(64);
    let link = "b".repeat(64);
    assert!(entry.live_link(&agent, 0).is_none());
    assert!(entry.link_role(&agent, 0).is_none());
    assert!(!entry.names("", &agent, 0));
    assert_eq!(entry.link_role(&link, 0), Some(Role::Commenter));
    assert!(entry.live_agent(&agent, 0).is_some());
    assert!(entry.live_agent(&link, 0).is_none());
}

#[test]
fn several_links_of_one_role_live_side_by_side() {
    let mut entry = IndexEntry::default();
    entry.add_link(grant('a', "reader", LinkKind::Link));
    entry.add_link(grant('b', "reader", LinkKind::Link));
    assert_eq!(entry.links.len(), 2);
    assert!(entry.link_role(&"a".repeat(64), 0).is_some());
    assert!(entry.link_role(&"b".repeat(64), 0).is_some());
}

#[test]
fn replace_swaps_one_row_and_the_old_key_stops_answering() {
    let mut entry = IndexEntry::default();
    entry.add_link(grant('a', "reader", LinkKind::Link));
    entry.add_link(grant('b', "reader", LinkKind::Link));
    let id = "a".repeat(16);
    assert!(entry.replace_link(&id, grant('c', "reader", LinkKind::Link)));
    assert!(entry.link_role(&"a".repeat(64), 0).is_none());
    assert!(entry.link_role(&"c".repeat(64), 0).is_some());
    assert!(entry.link_role(&"b".repeat(64), 0).is_some());
    assert!(!entry.replace_link("0000000000000000", grant('d', "reader", LinkKind::Link)));
}

#[test]
fn revoke_removes_one_row_live_or_expired() {
    let mut entry = IndexEntry::default();
    entry.add_link(grant('a', "reader", LinkKind::Link));
    let mut expired = grant('b', "reader", LinkKind::Link);
    expired.until = "2000-01-01T00:00:00Z".into();
    entry.add_link(expired);
    assert!(entry.drop_link(&"b".repeat(16)));
    assert!(entry.drop_link(&"a".repeat(16)));
    assert!(!entry.drop_link(&"a".repeat(16)));
    assert!(entry.links.is_empty());
}

#[test]
fn guests_of_a_dropped_link_are_pruned_and_agents_never_hold_guests() {
    let mut entry = IndexEntry::default();
    entry.add_link(grant('a', "reader", LinkKind::Link));
    entry.add_link(grant('b', "reader", LinkKind::Link));
    entry.guests = vec![
        Guest {
            id: "1".into(),
            link: "a".repeat(64),
            ..Guest::default()
        },
        Guest {
            id: "2".into(),
            link: "b".repeat(64),
            ..Guest::default()
        },
    ];
    entry.drop_link(&"a".repeat(16));
    entry.prune_guests(0);
    assert_eq!(entry.guests.len(), 1);
    assert_eq!(entry.guests[0].id, "2");
}

#[test]
fn dropping_agents_keeps_the_links() {
    let mut entry = IndexEntry::default();
    entry.add_link(grant('a', "reader", LinkKind::Link));
    entry.add_link(grant('b', "commenter", LinkKind::Agent));
    entry.drop_agents();
    assert_eq!(entry.links.len(), 1);
    assert_eq!(entry.links[0].kind, LinkKind::Link);
}
