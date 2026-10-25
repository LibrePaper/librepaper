//! Sorting what Git said on a failed network command into the reasons the
//! panel acts on: the person has to sign in, the network is down, or the
//! repository is not there.

/// Why a command that talks to a remote failed.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Kind {
    /// The remote refused the person: no credential, a wrong one, or a key
    /// the host does not know.
    Auth,
    /// The remote could not be reached.
    Network,
    /// The remote answered and has no such repository.
    NotFound,
}

impl Kind {
    /// The `reason` the routes answer with.
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Kind::Auth => "auth",
            Kind::Network => "network",
            Kind::NotFound => "not-found",
        }
    }
}

/// What `said`, the text of a failed command, shows about the remote. `None`
/// when it is none of the three, for instance a refusal by a hook or by the
/// working tree, which is not the remote's doing.
pub(crate) fn classify(said: &str) -> Option<Kind> {
    let lower = said.to_lowercase();
    let has = |needles: &[&str]| needles.iter().any(|needle| lower.contains(needle));
    if has(&[
        "could not resolve host",
        "could not resolve hostname",
        "connection refused",
        "timed out",
        "network is unreachable",
        "no route to host",
        "unable to connect",
        "failed to connect",
        "connection reset",
        "ssl certificate",
    ]) {
        Some(Kind::Network)
    } else if has(&[
        "authentication failed",
        "permission denied",
        "could not read username",
        "could not read password",
        "terminal prompts disabled",
        "host key verification failed",
        "access denied",
        "invalid credentials",
        "invalid username or password",
        "returned error: 401",
        "returned error: 403",
    ]) {
        Some(Kind::Auth)
    } else if has(&[
        "repository not found",
        "does not appear to be a git repository",
        "returned error: 404",
    ]) {
        Some(Kind::NotFound)
    } else {
        None
    }
}

/// Whether a push was refused because the remote branch has commits the
/// local one does not have.
pub(crate) fn behind(said: &str) -> bool {
    let lower = said.to_lowercase();
    ["non-fast-forward", "fetch first", "stale info"]
        .iter()
        .any(|needle| lower.contains(needle))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_missing_or_refused_credential_is_auth() {
        for said in [
            "fatal: could not read Username for 'https://github.com': terminal prompts disabled",
            "remote: Invalid username or password.\nfatal: Authentication failed for 'https://github.com/o/r.git/'",
            "git@github.com: Permission denied (publickey).\nfatal: Could not read from remote repository.\n\nPlease make sure you have the correct access rights\nand the repository exists.",
            "Host key verification failed.\nfatal: Could not read from remote repository.",
            "fatal: unable to access 'https://github.com/o/r.git/': The requested URL returned error: 403",
        ] {
            assert_eq!(classify(said), Some(Kind::Auth), "{said}");
        }
    }

    #[test]
    fn an_unreachable_host_is_network() {
        for said in [
            "fatal: unable to access 'https://github.com/o/r.git/': Could not resolve host: github.com",
            "ssh: connect to host github.com port 22: Connection timed out",
            "fatal: unable to access 'http://127.0.0.1:1/x.git/': Failed to connect to 127.0.0.1 port 1: Connection refused",
            "ssh: Could not resolve hostname github.com: Name or service not known",
            "fatal: unable to access 'https://x/': SSL certificate problem: self-signed certificate",
        ] {
            assert_eq!(classify(said), Some(Kind::Network), "{said}");
        }
    }

    #[test]
    fn a_missing_repository_is_not_found() {
        for said in [
            "remote: Repository not found.\nfatal: repository 'https://github.com/o/r.git/' not found",
            "fatal: '/tmp/nope' does not appear to be a git repository\nfatal: Could not read from remote repository.",
            "fatal: unable to access 'https://x/': The requested URL returned error: 404",
        ] {
            assert_eq!(classify(said), Some(Kind::NotFound), "{said}");
        }
    }

    #[test]
    fn what_is_not_the_remotes_doing_is_not_classified() {
        for said in [
            "error: Your local changes to the following files would be overwritten by merge:\n\ta.txt",
            "fatal: refusing to merge unrelated histories",
            "error: failed to push some refs to 'origin'",
            "",
        ] {
            assert_eq!(classify(said), None, "{said}");
        }
    }

    #[test]
    fn kinds_have_the_names_the_routes_answer_with() {
        assert_eq!(Kind::Auth.as_str(), "auth");
        assert_eq!(Kind::Network.as_str(), "network");
        assert_eq!(Kind::NotFound.as_str(), "not-found");
    }

    #[test]
    fn a_push_behind_the_remote_is_recognised() {
        for said in [
            " ! [rejected]        main -> main (non-fast-forward)",
            " ! [rejected]        main -> main (fetch first)\nhint: Updates were rejected because the remote contains work that you do not have locally.",
            "! [rejected] main -> main (stale info)",
        ] {
            assert!(behind(said), "{said}");
        }
        assert!(!behind("fatal: Authentication failed"));
        assert!(!behind(
            " ! [remote rejected] main -> main (pre-receive hook declined)"
        ));
    }
}
