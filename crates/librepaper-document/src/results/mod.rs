//! Document metadata shared by the server and engine.
//!
//! `ExecutionEngine` and `DraftFormat` name how a stored `source_format` is
//! executed and edited. They are the only vocabulary here.

use serde::{Deserialize, Serialize};

/// Engines that may own an executable document.  Calepin and unknown values
/// are intentionally absent: serde rejects them at the JSON boundary rather
/// than silently treating them as Quarto.
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum ExecutionEngine {
    #[default]
    None,
    Quarto,
}

impl ExecutionEngine {
    pub fn parse(value: &str) -> Result<Self, String> {
        match value {
            "none" => Ok(Self::None),
            "quarto" => Ok(Self::Quarto),
            other => Err(format!("unsupported execution engine: {other}")),
        }
    }
}

/// The editable draft representation is independent of the execution engine.
/// In particular, a legacy Quarto source is a Markdown draft with Quarto
/// execution metadata, rather than a new draft format named `quarto`.
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum DraftFormat {
    #[default]
    Markdown,
    Html,
    Typst,
    Latex,
    Other,
}

impl DraftFormat {
    pub fn from_source_format(source_format: &str) -> Self {
        match source_format {
            "markdown" | "quarto" => Self::Markdown,
            "" => Self::Html,
            "html" => Self::Html,
            "typst" => Self::Typst,
            "latex" => Self::Latex,
            _ => Self::Other,
        }
    }

    pub fn parse(value: &str) -> Result<Self, String> {
        match value {
            "markdown" => Ok(Self::Markdown),
            "html" => Ok(Self::Html),
            "typst" => Ok(Self::Typst),
            "latex" => Ok(Self::Latex),
            "other" => Ok(Self::Other),
            other => Err(format!("unsupported draft format: {other}")),
        }
    }
}

/// Explicit document metadata used at API boundaries.  The
/// persisted `source_format` remains the compatibility wire value.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DocumentMetadata {
    pub execution_engine: ExecutionEngine,
    pub draft_format: DraftFormat,
}

impl DocumentMetadata {
    pub fn from_source_format(source_format: &str) -> Self {
        Self {
            execution_engine: if source_format == "quarto" {
                ExecutionEngine::Quarto
            } else {
                ExecutionEngine::None
            },
            draft_format: DraftFormat::from_source_format(source_format),
        }
    }
}

/// Normalize legacy document metadata for API consumers that do not have a
/// catalogue handle. The catalogue migration stores the same pair durably.
pub fn document_metadata(source_format: &str) -> DocumentMetadata {
    DocumentMetadata::from_source_format(source_format)
}
