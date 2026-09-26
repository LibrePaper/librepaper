//! Approval dialogs for permission requests from the web.
//!
//! This stub is a placeholder. The full implementation will be provided by another agent.

#[derive(Clone, Debug)]
pub(crate) struct Approval {
    pub title: String,
    pub message: String,
    pub allow_label: String,
}

#[derive(Clone, Copy, Debug)]
pub(crate) enum Decision {
    Allowed,
    Denied,
    Unavailable(String),
}

impl Decision {
    fn from_str(s: &str) -> Self {
        match s {
            "unavailable" => Decision::Unavailable("approval system unavailable".to_string()),
            _ => Decision::Denied,
        }
    }
}

pub(crate) async fn ask(approval: &Approval) -> Decision {
    #[cfg(test)]
    {
        if let Ok(val) = std::env::var("LIBREPAPER_TEST_APPROVAL") {
            return Decision::from_str(&val);
        }
    }
    Decision::Denied
}

pub(crate) fn approve_code(code: &str) -> bool {
    #[cfg(test)]
    {
        if let Ok(val) = std::env::var("LIBREPAPER_APPROVAL_CODE") {
            return val == code;
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn approval_respects_test_env() {
        std::env::set_var("LIBREPAPER_TEST_APPROVAL", "allowed");
        let approval = Approval {
            title: "Test".to_string(),
            message: "Test message".to_string(),
            allow_label: "Allow".to_string(),
        };
        match ask(&approval).await {
            Decision::Allowed => {}
            _ => panic!("expected Allowed"),
        }
    }
}
