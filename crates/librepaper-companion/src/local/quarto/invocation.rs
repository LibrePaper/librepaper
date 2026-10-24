//! The checked Quarto command plan.

use std::path::Path;

use serde_json::Value;
use tokio::process::Command;

use crate::local::protocol::QuartoJobOptions;

/// A checked Quarto command plan.  Paths are supplied only by the local
/// service when the plan is applied; browser values remain typed options and
/// cannot become an executable or shell fragment.
#[derive(Clone, Debug, PartialEq)]
pub struct QuartoInvocationPlan {
    pub main: String,
    pub format: String,
    pub render_scope: crate::local::protocol::QuartoRenderScope,
    pub policy: crate::local::protocol::QuartoRenderPolicy,
    pub profile: Option<String>,
    pub parameters: Vec<(String, Value)>,
}

impl QuartoInvocationPlan {
    pub fn from_options(options: &QuartoJobOptions) -> Result<Self, String> {
        options.validate()?;
        Ok(Self {
            main: options.main.clone(),
            format: options.format.clone(),
            render_scope: options.render_scope,
            policy: options.policy,
            profile: options.profile.clone(),
            parameters: options
                .parameters
                .iter()
                .map(|(name, value)| (name.clone(), value.clone()))
                .collect(),
        })
    }

    /// Appends the fixed argument vector used by the Quarto adapter.  The
    /// caller has already canonicalised the project root and checked the
    /// entrypoint against its binding.
    pub fn apply(&self, command: &mut Command, output: &Path, filter: &Path) {
        command.arg("render");
        if self.render_scope == crate::local::protocol::QuartoRenderScope::Document {
            command.arg(&self.main);
        }
        command
            .arg("--to")
            .arg(&self.format)
            .arg("--no-execute-daemon")
            .arg("--output-dir")
            .arg(output)
            .arg("--lua-filter")
            .arg(filter);
        if html_output(&self.format) {
            // Published documents may run their own code and may not fetch code
            // from another host, which is the reader policy in
            // `server::routes::document_policy`. Quarto's default HTML output
            // loads MathJax and a polyfill from public delivery networks, so a
            // document rendered without this would lose its mathematics for
            // every reader and would tell those networks who was reading it.
            //
            // Forced rather than defaulted. An author who sets
            // `embed-resources: false` is asking for output this deployment
            // cannot serve intact, so honoring it would only produce a broken
            // paper.
            command.arg("--standalone").arg("--embed-resources");
        }
        match self.policy {
            crate::local::protocol::QuartoRenderPolicy::ProjectDefaults => {}
            crate::local::protocol::QuartoRenderPolicy::RefreshComputations => {
                command.arg("--cache-refresh");
            }
            crate::local::protocol::QuartoRenderPolicy::Frozen => {
                // `--use-freezer` selects the saved computation result. The
                // adapter's preflight verifies the complete cache and source
                // identity before this process starts; adding `--no-execute`
                // here makes Quarto drop cached display nodes in 1.10.18.
                command.arg("--use-freezer");
            }
        }
        if let Some(profile) = &self.profile {
            command.arg("--profile").arg(profile);
        }
        for (name, value) in &self.parameters {
            let value = serde_json::to_string(value).unwrap_or_else(|_| "null".into());
            command.arg("-P").arg(format!("{name}:{value}"));
        }
    }
}

/// Whether a Quarto format produces a web page, and so has to carry its own
/// resources. `pdf` and `docx` are self-contained by construction.
fn html_output(format: &str) -> bool {
    matches!(format, "html" | "revealjs")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rendered_args(format: &str) -> Vec<String> {
        let plan = QuartoInvocationPlan {
            main: "paper.qmd".into(),
            format: format.into(),
            render_scope: crate::local::protocol::QuartoRenderScope::Document,
            policy: crate::local::protocol::QuartoRenderPolicy::ProjectDefaults,
            profile: None,
            parameters: Vec::new(),
        };
        let mut command = Command::new("quarto");
        plan.apply(
            &mut command,
            Path::new("/tmp/out"),
            Path::new("/tmp/filter.lua"),
        );
        command
            .as_std()
            .get_args()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect()
    }

    /// A web page has to carry its own scripts, because the reader policy
    /// refuses code fetched from another host. Without this, Quarto's default
    /// output loses its mathematics to a blocked delivery network.
    #[test]
    fn web_output_is_rendered_self_contained() {
        for format in ["html", "revealjs"] {
            let args = rendered_args(format);
            assert!(
                args.iter().any(|arg| arg == "--embed-resources"),
                "{format} must embed its resources: {args:?}"
            );
            assert!(args.iter().any(|arg| arg == "--standalone"), "{format}");
        }
    }

    /// Not every format is a web page. Embedding is meaningless for these and
    /// Quarto would have to be told to ignore it.
    #[test]
    fn other_formats_are_left_alone() {
        for format in ["pdf", "docx"] {
            let args = rendered_args(format);
            assert!(
                !args.iter().any(|arg| arg == "--embed-resources"),
                "{format} should not be told to embed: {args:?}"
            );
        }
    }

    #[test]
    fn invocation_plan_preserves_typed_policy_and_parameters() {
        let mut options = QuartoJobOptions {
            binding_id: "binding".into(),
            main: "paper.qmd".into(),
            ..Default::default()
        };
        options
            .parameters
            .insert("answer".into(), serde_json::json!(42));
        let plan = QuartoInvocationPlan::from_options(&options).expect("valid options");
        assert_eq!(plan.main, "paper.qmd");
        assert_eq!(
            plan.parameters,
            vec![("answer".into(), serde_json::json!(42))]
        );
    }

    #[test]
    fn policy_capabilities_are_version_gated() {
        assert_eq!(
            crate::local::quarto::supported_policies(Some("Quarto 1.2.9")),
            vec!["project-defaults"]
        );
        assert!(crate::local::quarto::supported_policies(Some("Quarto 1.3.0")).contains(&"frozen".into()));
        assert_eq!(crate::local::quarto::supported_policies(None), vec!["project-defaults"]);
    }
}
