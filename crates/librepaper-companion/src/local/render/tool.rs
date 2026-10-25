//! The tool record: one command and one result file per format and output.
//!
//! The page never sends a command. It names a format and an output, and the
//! sidecar picks the record: the user's own edit when there is one, and
//! otherwise the built-in default from the table below. Two placeholders
//! exist, `{input}` and `{stem}`; every other `{...}` is refused when a
//! record is saved.

use serde::{Deserialize, Serialize};

use crate::local::protocol::safe_relative_path;

/// Most arguments a command may hold.
const MAX_ARGUMENTS: usize = 64;
/// Most bytes one argument, or the result path, may hold.
const MAX_ARGUMENT_BYTES: usize = 4096;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Format {
    Latex,
    Typst,
    Markdown,
    Quarto,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Output {
    Pdf,
    Html,
    Docx,
}

impl Format {
    fn parse(value: &str) -> Result<Self, String> {
        match value {
            "latex" => Ok(Self::Latex),
            "typst" => Ok(Self::Typst),
            "markdown" => Ok(Self::Markdown),
            "quarto" => Ok(Self::Quarto),
            _ => Err(format!(
                "unknown format: {value} (supported: latex, typst, markdown, quarto)"
            )),
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Latex => "latex",
            Self::Typst => "typst",
            Self::Markdown => "markdown",
            Self::Quarto => "quarto",
        }
    }
}

impl Output {
    fn parse(value: &str) -> Result<Self, String> {
        match value {
            "pdf" => Ok(Self::Pdf),
            "html" => Ok(Self::Html),
            "docx" => Ok(Self::Docx),
            _ => Err(format!(
                "unknown output: {value} (supported: pdf, html, docx)"
            )),
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Pdf => "pdf",
            Self::Html => "html",
            Self::Docx => "docx",
        }
    }
}

/// A format and an output that have a tool record. A pair with no default
/// cannot be built, so a `Key` in hand always names a record.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Key {
    pub format: Format,
    pub output: Output,
}

impl Key {
    /// Parses `"<format>.<output>"`, the key the wire uses.
    pub fn parse(value: &str) -> Result<Self, String> {
        let (format, output) = value
            .split_once('.')
            .ok_or_else(|| format!("a renderer is named <format>.<output>, not {value}"))?;
        Self::parse_parts(format, output)
    }

    pub fn parse_parts(format: &str, output: &str) -> Result<Self, String> {
        let key = Key {
            format: Format::parse(format)?,
            output: Output::parse(output)?,
        };
        if default_record(key).is_none() {
            return Err(format!("{format} has no {output} renderer"));
        }
        Ok(key)
    }

    /// Every key that has a default record, in a stable order.
    pub fn all() -> Vec<Key> {
        let mut keys: Vec<Key> = DEFAULTS
            .iter()
            .map(|(format, output, _, _)| Key {
                format: *format,
                output: *output,
            })
            .collect();
        keys.sort();
        keys
    }
}

impl std::fmt::Display for Key {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            formatter,
            "{}.{}",
            self.format.as_str(),
            self.output.as_str()
        )
    }
}

/// One command and the file it leaves behind.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Record {
    pub command: Vec<String>,
    pub result: String,
}

/// A record with its placeholders replaced for one entry file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Expanded {
    pub command: Vec<String>,
    /// The result path relative to the workspace root.
    pub result: String,
    /// The entry path without its extension, which names the SyncTeX files.
    pub stem: String,
}

const QUARTO_HTML: &[&str] = &[
    "quarto",
    "render",
    "{input}",
    "--to",
    "html",
    "--embed-resources",
    "--standalone",
];
const QUARTO_PDF: &[&str] = &["quarto", "render", "{input}", "--to", "pdf"];
const QUARTO_DOCX: &[&str] = &["quarto", "render", "{input}", "--to", "docx"];

/// The built-in defaults. A record the user never edited is not written to
/// disk, so a later release can change a row here.
const DEFAULTS: &[(Format, Output, &[&str], &str)] = &[
    (
        Format::Latex,
        Output::Pdf,
        &[
            "latexmk",
            "-pdf",
            "-cd",
            "-synctex=1",
            "-interaction=nonstopmode",
            "-file-line-error",
            "{input}",
        ],
        "{stem}.pdf",
    ),
    (
        Format::Typst,
        Output::Pdf,
        &[
            "typst",
            "compile",
            "--root",
            ".",
            "--font-path",
            ".",
            "{input}",
            "{stem}.pdf",
        ],
        "{stem}.pdf",
    ),
    (Format::Quarto, Output::Html, QUARTO_HTML, "{stem}.html"),
    (Format::Quarto, Output::Pdf, QUARTO_PDF, "{stem}.pdf"),
    (Format::Quarto, Output::Docx, QUARTO_DOCX, "{stem}.docx"),
    (Format::Markdown, Output::Html, QUARTO_HTML, "{stem}.html"),
    (Format::Markdown, Output::Pdf, QUARTO_PDF, "{stem}.pdf"),
    (Format::Markdown, Output::Docx, QUARTO_DOCX, "{stem}.docx"),
];

/// The built-in record for `key`, if there is one.
pub fn default_record(key: Key) -> Option<Record> {
    DEFAULTS
        .iter()
        .find(|(format, output, _, _)| *format == key.format && *output == key.output)
        .map(|(_, _, command, result)| Record {
            command: command.iter().map(|argument| (*argument).into()).collect(),
            result: (*result).into(),
        })
}

impl Record {
    /// Checks a record before it is saved: the command is a non-empty
    /// argument vector within the bounds, every placeholder is one of the two
    /// known ones, and the result expands to a safe relative path.
    pub fn validate(&self) -> Result<(), String> {
        if self.command.is_empty() {
            return Err("the command is empty".into());
        }
        if self.command.len() > MAX_ARGUMENTS {
            return Err(format!(
                "too many args: {} (maximum {MAX_ARGUMENTS})",
                self.command.len()
            ));
        }
        for (i, argument) in self.command.iter().enumerate() {
            if argument.is_empty() {
                return Err(format!("arg #{i} is empty"));
            }
            if argument.len() > MAX_ARGUMENT_BYTES {
                return Err(format!(
                    "arg #{i} is too long: {} bytes (maximum {MAX_ARGUMENT_BYTES})",
                    argument.len()
                ));
            }
            if argument.contains('\0') {
                return Err(format!("arg #{i} contains NUL byte"));
            }
        }
        if self.result.len() > MAX_ARGUMENT_BYTES {
            return Err(format!(
                "the result path is too long: {} bytes (maximum {MAX_ARGUMENT_BYTES})",
                self.result.len()
            ));
        }
        // Any safe entry file shows whether the placeholders are known and
        // whether the result stays inside the workspace.
        self.expand("main.tex").map(|_| ())
    }

    /// Replaces `{input}` and `{stem}` for the entry file `input`, a safe
    /// path relative to the workspace root.
    pub fn expand(&self, input: &str) -> Result<Expanded, String> {
        let stem = stem_of(input);
        let command = self
            .command
            .iter()
            .map(|argument| expand_template(argument, input, stem))
            .collect::<Result<Vec<_>, _>>()?;
        let result = expand_template(&self.result, input, stem)?;
        if !safe_relative_path(&result) {
            return Err(format!(
                "the result path must be a safe relative path, not {result}"
            ));
        }
        Ok(Expanded {
            command,
            result,
            stem: stem.to_string(),
        })
    }
}

/// The path without the extension of its last component. A leading dot is a
/// name, not an extension.
pub fn stem_of(input: &str) -> &str {
    let name_start = input.rfind('/').map_or(0, |slash| slash + 1);
    match input[name_start..].rfind('.') {
        Some(dot) if dot > 0 => &input[..name_start + dot],
        _ => input,
    }
}

fn expand_template(template: &str, input: &str, stem: &str) -> Result<String, String> {
    let mut out = String::with_capacity(template.len());
    let mut rest = template;
    while let Some(open) = rest.find('{') {
        // A brace that never closes is text, not a placeholder.
        let Some(close) = rest[open..].find('}') else {
            break;
        };
        out.push_str(&rest[..open]);
        let name = &rest[open + 1..open + close];
        match name {
            "input" => out.push_str(input),
            "stem" => out.push_str(stem),
            _ => {
                return Err(format!(
                    "unknown placeholder {{{name}}}: only {{input}} and {{stem}} are allowed"
                ))
            }
        }
        rest = &rest[open + close + 1..];
    }
    out.push_str(rest);
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record(command: &[&str], result: &str) -> Record {
        Record {
            command: command.iter().map(|argument| (*argument).into()).collect(),
            result: result.into(),
        }
    }

    #[test]
    fn keys_parse_in_the_wire_form_and_print_back() {
        let key = Key::parse("latex.pdf").unwrap();
        assert_eq!(key.format, Format::Latex);
        assert_eq!(key.output, Output::Pdf);
        assert_eq!(key.to_string(), "latex.pdf");
        for bad in ["latex", "latex.", ".pdf", "pandoc.pdf", "latex.odt", "a.b.c", ""] {
            assert!(Key::parse(bad).is_err(), "{bad:?} parsed");
        }
    }

    #[test]
    fn a_pair_with_no_record_is_refused() {
        assert!(Key::parse("latex.html").is_err());
        assert!(Key::parse("latex.docx").is_err());
        assert!(Key::parse("typst.html").is_err());
        assert!(Key::parse_parts("typst", "docx").is_err());
        Key::parse_parts("markdown", "docx").unwrap();
    }

    #[test]
    fn the_default_table_holds_exactly_the_documented_records() {
        let names: Vec<String> = Key::all().iter().map(Key::to_string).collect();
        assert_eq!(
            names,
            [
                "latex.pdf",
                "typst.pdf",
                "markdown.pdf",
                "markdown.html",
                "markdown.docx",
                "quarto.pdf",
                "quarto.html",
                "quarto.docx",
            ]
        );
        let latex = default_record(Key::parse("latex.pdf").unwrap()).unwrap();
        assert_eq!(
            latex.command,
            [
                "latexmk",
                "-pdf",
                "-cd",
                "-synctex=1",
                "-interaction=nonstopmode",
                "-file-line-error",
                "{input}"
            ]
        );
        assert_eq!(latex.result, "{stem}.pdf");
        let typst = default_record(Key::parse("typst.pdf").unwrap()).unwrap();
        assert_eq!(
            typst.command,
            [
                "typst",
                "compile",
                "--root",
                ".",
                "--font-path",
                ".",
                "{input}",
                "{stem}.pdf"
            ]
        );
        let html = default_record(Key::parse("quarto.html").unwrap()).unwrap();
        assert_eq!(
            html.command,
            [
                "quarto",
                "render",
                "{input}",
                "--to",
                "html",
                "--embed-resources",
                "--standalone"
            ]
        );
        for output in ["pdf", "html", "docx"] {
            assert_eq!(
                default_record(Key::parse(&format!("markdown.{output}")).unwrap()),
                default_record(Key::parse(&format!("quarto.{output}")).unwrap()),
                "markdown.{output} copies quarto.{output}"
            );
        }
    }

    #[test]
    fn every_default_is_a_valid_record() {
        for key in Key::all() {
            default_record(key).unwrap().validate().unwrap();
        }
    }

    #[test]
    fn input_and_stem_expand_for_a_nested_entry_file() {
        let expanded = default_record(Key::parse("latex.pdf").unwrap())
            .unwrap()
            .expand("chapters/main.tex")
            .unwrap();
        assert_eq!(expanded.command.last().unwrap(), "chapters/main.tex");
        assert_eq!(expanded.result, "chapters/main.pdf");
        assert_eq!(expanded.stem, "chapters/main");
        let typst = default_record(Key::parse("typst.pdf").unwrap())
            .unwrap()
            .expand("paper.typ")
            .unwrap();
        assert_eq!(&typst.command[6..], ["paper.typ", "paper.pdf"]);
    }

    #[test]
    fn a_stem_drops_only_the_last_extension_of_the_last_component() {
        assert_eq!(stem_of("main.tex"), "main");
        assert_eq!(stem_of("a.b/main"), "a.b/main");
        assert_eq!(stem_of("a.b/main.x.tex"), "a.b/main.x");
        assert_eq!(stem_of(".hidden"), ".hidden");
        assert_eq!(stem_of("dir/.hidden"), "dir/.hidden");
    }

    #[test]
    fn an_unknown_placeholder_is_refused_when_saved() {
        for bad in ["{path}", "{INPUT}", "{}", "{input }", "--x={home}"] {
            let error = record(&["tool", bad], "{stem}.pdf").validate().unwrap_err();
            assert!(error.contains("placeholder"), "{bad}: {error}");
        }
        let error = record(&["tool"], "{out}.pdf").validate().unwrap_err();
        assert!(error.contains("placeholder"), "{error}");
        record(&["tool", "--x={input}", "{stem}"], "{stem}.pdf")
            .validate()
            .unwrap();
        // A brace that never closes is plain text.
        record(&["tool", "a{b"], "{stem}.pdf").validate().unwrap();
    }

    #[test]
    fn the_result_must_expand_to_a_safe_relative_path() {
        for bad in ["", "../out.pdf", "/tmp/out.pdf", "out/../../x.pdf", "a\\b.pdf", "C:/x.pdf"] {
            assert!(
                record(&["tool"], bad).validate().is_err(),
                "{bad:?} was accepted"
            );
        }
        record(&["tool"], "build/{stem}.pdf").validate().unwrap();
    }

    #[test]
    fn the_argument_bounds_hold() {
        assert!(record(&[], "{stem}.pdf").validate().is_err());
        assert!(Record {
            command: vec!["x".into(); 65],
            result: "{stem}.pdf".into()
        }
        .validate()
        .is_err());
        assert!(record(&["good", ""], "{stem}.pdf").validate().is_err());
        assert!(record(&["good", "bad\0arg"], "{stem}.pdf").validate().is_err());
        let long = "x".repeat(4097);
        assert!(record(&[long.as_str()], "{stem}.pdf").validate().is_err());
        assert!(Record {
            command: vec!["x".into(); 64],
            result: "{stem}.pdf".into()
        }
        .validate()
        .is_ok());
    }
}
