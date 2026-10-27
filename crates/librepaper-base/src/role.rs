//! What a caller may do to a document, as one ladder, and the switches that
//! bound it.

use std::fmt;
use std::str::FromStr;

/// What a caller may do to a document, as a ladder: each role includes the
/// ones beneath it. Reading has never had a switch, so `Reader` is what
/// everybody who can reach a document holds; `Editor` sits between commenting
/// and owning and is held today by the owner alone, until a document can name
/// somebody.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Role {
    Reader,
    Commenter,
    Editor,
    Owner,
}

impl Role {
    /// Whether this role includes another: the ladder, asked as a question.
    pub fn at_least(self, wanted: Role) -> bool {
        self >= wanted
    }

    /// The spelling that is stored and returned by the API.
    pub fn as_str(self) -> &'static str {
        match self {
            Role::Reader => "reader",
            Role::Commenter => "commenter",
            Role::Editor => "editor",
            Role::Owner => "owner",
        }
    }

    /// The byte an agent grant carries. A grant is a ceiling for an agent and
    /// never names an owner, so `Owner` shares the editor byte.
    pub fn wire(self) -> u8 {
        match self {
            Role::Reader => 1,
            Role::Commenter => 2,
            Role::Editor | Role::Owner => 3,
        }
    }
}

/// A word that names no role a document hands out.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct UnknownRole;

impl fmt::Display for UnknownRole {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("not a role")
    }
}

impl std::error::Error for UnknownRole {}

impl TryFrom<u8> for Role {
    type Error = UnknownRole;

    /// The inverse of `wire`, for the three bytes a grant may carry.
    fn try_from(byte: u8) -> Result<Role, UnknownRole> {
        match byte {
            1 => Ok(Role::Reader),
            2 => Ok(Role::Commenter),
            3 => Ok(Role::Editor),
            _ => Err(UnknownRole),
        }
    }
}

impl FromStr for Role {
    type Err = UnknownRole;

    /// The role a grant or a link names. The long forms are what is stored,
    /// and the panel and the command line also offer the short verbs
    /// `read`, `comment` and `edit`. Nobody is granted `owner`, since the
    /// owner is one account, so that word is not accepted.
    fn from_str(value: &str) -> Result<Role, UnknownRole> {
        match value.trim().to_ascii_lowercase().as_str() {
            "read" | "reader" => Ok(Role::Reader),
            "comment" | "commenter" => Ok(Role::Commenter),
            "edit" | "editor" => Ok(Role::Editor),
            _ => Err(UnknownRole),
        }
    }
}

/// What each rung of the ladder may do.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Capabilities {
    pub read: bool,
    pub comment: bool,
    pub edit: bool,
    pub share: bool,
    /// Deleting anyone's comment, which the editor rung holds.
    pub moderate: bool,
}

impl From<Role> for Capabilities {
    fn from(role: Role) -> Capabilities {
        Capabilities {
            read: true,
            comment: role.at_least(Role::Commenter),
            edit: role.at_least(Role::Editor),
            share: role.at_least(Role::Owner),
            moderate: role.at_least(Role::Editor),
        }
    }
}

/// The most a document may open each role to one caller, which is what the
/// deployment's TOML settings say. The `publishers` and `commenters` lists
/// under the `[auth.*]` tables are ceilings: a document may only ever be
/// stricter than its server, so a grant recorded while a switch was wide
/// stops answering when the switch narrows.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Ceiling {
    /// Whether this caller may comment here at all.
    pub comment: bool,
    /// Whether this caller may publish here, and so be named an editor. A
    /// link names nobody, so it is asked with an empty handle, and publishing
    /// always asks for a sign-in first, so a link never edits.
    pub edit: bool,
}

impl Ceiling {
    /// What a granted role comes to under the switches, for a caller bounded
    /// by a row alone: an editor row edits under the publish switch,
    /// comments under the comment switch and otherwise reads. An owner is
    /// never granted, so that rung reads.
    pub fn clamp(&self, granted: Role) -> Role {
        match granted {
            Role::Editor if self.edit => Role::Editor,
            Role::Editor | Role::Commenter if self.comment => Role::Commenter,
            _ => Role::Reader,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const LADDER: [Role; 4] = [Role::Reader, Role::Commenter, Role::Editor, Role::Owner];

    #[test]
    fn ladder_is_ordered() {
        for (i, low) in LADDER.iter().enumerate() {
            for (j, high) in LADDER.iter().enumerate() {
                assert_eq!(high.at_least(*low), j >= i);
            }
        }
    }

    #[test]
    fn every_alias_parses() {
        for (word, role) in [
            ("read", Role::Reader),
            ("reader", Role::Reader),
            ("comment", Role::Commenter),
            ("commenter", Role::Commenter),
            ("edit", Role::Editor),
            ("editor", Role::Editor),
            (" Editor ", Role::Editor),
            ("READ", Role::Reader),
        ] {
            assert_eq!(word.parse::<Role>(), Ok(role), "{word}");
        }
        for word in ["owner", "", "admin", "viewer"] {
            assert_eq!(word.parse::<Role>(), Err(UnknownRole), "{word}");
        }
    }

    #[test]
    fn spelling_round_trips() {
        for role in [Role::Reader, Role::Commenter, Role::Editor] {
            assert_eq!(role.as_str().parse::<Role>(), Ok(role));
        }
    }

    #[test]
    fn wire_bytes() {
        for byte in 1..=3u8 {
            assert_eq!(Role::try_from(byte).unwrap().wire(), byte);
        }
        assert_eq!(Role::Owner.wire(), 3);
        for byte in [0u8, 4, 5, 255] {
            assert_eq!(Role::try_from(byte), Err(UnknownRole));
        }
    }

    #[test]
    fn capabilities_follow_the_ladder() {
        let caps = |role| Capabilities::from(role);
        assert!(caps(Role::Reader).read && !caps(Role::Reader).comment);
        assert!(caps(Role::Commenter).comment && !caps(Role::Commenter).edit);
        assert!(caps(Role::Editor).edit && caps(Role::Editor).moderate);
        assert!(!caps(Role::Editor).share);
        assert!(caps(Role::Owner).share);
    }

    #[test]
    fn clamp_matrix() {
        let rows = [
            // (edit, comment), then the result for reader, commenter, editor, owner.
            (
                (false, false),
                [Role::Reader, Role::Reader, Role::Reader, Role::Reader],
            ),
            (
                (false, true),
                [Role::Reader, Role::Commenter, Role::Commenter, Role::Reader],
            ),
            (
                (true, false),
                [Role::Reader, Role::Reader, Role::Editor, Role::Reader],
            ),
            (
                (true, true),
                [Role::Reader, Role::Commenter, Role::Editor, Role::Reader],
            ),
        ];
        for ((edit, comment), expected) in rows {
            let ceiling = Ceiling { edit, comment };
            for (granted, want) in LADDER.iter().zip(expected) {
                assert_eq!(
                    ceiling.clamp(*granted),
                    want,
                    "{edit} {comment} {granted:?}"
                );
            }
        }
    }
}
