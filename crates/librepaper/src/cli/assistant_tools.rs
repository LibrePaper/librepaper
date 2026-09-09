//! Focused reads for an assistant, all bounded by the document peer's access.

use std::collections::BTreeSet;

use clap::Subcommand;
use serde_json::{json, Value};

use super::peer::{AutomationPeer, Snapshot};

#[derive(Subcommand, Clone, Debug)]
pub enum InspectCommand {
    /// List document files without their contents.
    Files,
    /// List Markdown/Typst headings or LaTeX section commands.
    Headings {
        #[arg(long, default_value = "")]
        path: String,
    },
    /// Read a range of source lines (one-based, inclusive; at most 500).
    Section {
        #[arg(long, default_value = "")]
        path: String,
        #[arg(long, default_value_t = 1)]
        line: usize,
        #[arg(long)]
        end_line: Option<usize>,
    },
    /// Search literal text, returning source locations and captured anchors.
    Search {
        query: String,
        #[arg(long, default_value = "")]
        path: String,
        #[arg(long, default_value_t = 30, value_parser = clap::value_parser!(u32).range(1..=100))]
        limit: u32,
    },
    /// Read one comment and all of its replies.
    Thread { id: String },
    /// Read bibliography source files; use --path to select one.
    Bibliography {
        #[arg(long, default_value = "")]
        path: String,
    },
    /// Compare the live source with a saved checkpoint's full SHA.
    Changes { revision: String },
}

pub async fn inspect(peer: &AutomationPeer, command: InspectCommand) -> Result<Value, String> {
    let snapshot = peer.snapshot().await?;
    if let InspectCommand::Changes { revision } = &command {
        super::suggest::revision_value(revision)?;
        let endpoint = format!(
            "{}/api/documents/{}/history/{revision}",
            peer.link().server(),
            peer.link().slug()
        );
        let response = peer.request(reqwest::Method::GET, &endpoint, None).await?;
        if !response.status().is_success() {
            return Err(format!("checkpoint read failed ({})", response.status()));
        }
        let old: Value = response
            .json()
            .await
            .map_err(|e| format!("invalid checkpoint: {e}"))?;
        let before = old["texts"]
            .as_object()
            .ok_or("checkpoint has no source files")?;
        let after = snapshot
            .texts
            .as_object()
            .ok_or("snapshot has no source files")?;
        let paths: BTreeSet<_> = before.keys().chain(after.keys()).collect();
        let changes: Vec<_> = paths.into_iter().filter_map(|path| {
            let a = before.get(path).and_then(Value::as_str);
            let b = after.get(path).and_then(Value::as_str);
            (a != b).then(|| json!({"path":path,"diff":super::history::unified_source_diff(path,a.unwrap_or(""),b.unwrap_or(""),revision,&snapshot.sha,a.is_some(),b.is_some())}))
        }).collect();
        return Ok(json!({"revision":snapshot.sha,"from":revision,"changes":changes}));
    }
    inspect_snapshot(&snapshot, command)
}

fn selected<'a>(snapshot: &'a Snapshot, path: &'a str) -> Result<(&'a str, &'a str), String> {
    let path = if path.is_empty() {
        &snapshot.main
    } else {
        path
    };
    let source = snapshot
        .texts
        .get(path)
        .and_then(Value::as_str)
        .ok_or_else(|| format!("no text file {path:?}"))?;
    Ok((path, source))
}

fn inspect_snapshot(snapshot: &Snapshot, command: InspectCommand) -> Result<Value, String> {
    let files = snapshot
        .texts
        .as_object()
        .ok_or("snapshot has no source files")?;
    let content = match command {
        InspectCommand::Files => {
            json!({"main":snapshot.main,"files":snapshot.files,"text_files":files.iter().map(|(path,value)|json!({"path":path,"bytes":value.as_str().unwrap_or("").len()})).collect::<Vec<_>>()})
        }
        InspectCommand::Headings { path } => {
            let mut headings = Vec::new();
            if !path.is_empty() {
                selected(snapshot, &path)?;
            }
            for (name, value) in files {
                if !path.is_empty() && name != &path {
                    continue;
                }
                let mut fence = None;
                for (index, line) in value.as_str().unwrap_or("").lines().enumerate() {
                    let line = line.trim_start();
                    if line.starts_with("```") || line.starts_with("~~~") {
                        let marker = &line[..3];
                        if fence == Some(marker) {
                            fence = None;
                        } else if fence.is_none() {
                            fence = Some(marker);
                        }
                        continue;
                    }
                    if fence.is_some() {
                        continue;
                    }
                    let marker = if name.ends_with(".typ") { '=' } else { '#' };
                    let level = line.chars().take_while(|c| *c == marker).count();
                    if level > 0
                        && level <= 6
                        && line
                            .as_bytes()
                            .get(level)
                            .is_some_and(u8::is_ascii_whitespace)
                    {
                        headings.push(json!({"path":name,"line":index+1,"level":level,"text":line[level..].trim()}));
                    } else if name.ends_with(".tex") {
                        for (command, level) in [
                            ("\\chapter", 1),
                            ("\\section", 2),
                            ("\\subsection", 3),
                            ("\\subsubsection", 4),
                        ] {
                            if let Some(rest) = line.strip_prefix(command) {
                                let rest = rest.trim_start_matches('*');
                                if let Some(title) = rest
                                    .strip_prefix('{')
                                    .and_then(|s| s.split_once('}').map(|(title, _)| title))
                                {
                                    headings.push(json!({"path":name,"line":index+1,"level":level,"text":title}));
                                }
                            }
                        }
                    }
                }
            }
            json!({"headings":headings})
        }
        InspectCommand::Section {
            path,
            line,
            end_line,
        } => {
            let (path, source) = selected(snapshot, &path)?;
            let end = end_line.unwrap_or_else(|| line.saturating_add(99));
            if line == 0 || end < line || end - line >= 500 {
                return Err("choose 1 to 500 source lines with one-based line numbers".into());
            }
            let lines: Vec<_> = source.lines().collect();
            if line > lines.len().max(1) {
                return Err("starting line is past the end of the file".into());
            }
            json!({"path":path,"line":line,"end_line":end.min(lines.len()),"source":lines.iter().skip(line-1).take(end-line+1).copied().collect::<Vec<_>>().join("\n")})
        }
        InspectCommand::Search { query, path, limit } => {
            if query.is_empty() {
                return Err("search text cannot be empty".into());
            }
            if !path.is_empty() {
                selected(snapshot, &path)?;
            }
            let mut matches = Vec::new();
            let mut truncated = false;
            'files: for (name, value) in files {
                if !path.is_empty() && name != &path {
                    continue;
                }
                let source = value.as_str().unwrap_or("");
                for (offset, _) in source.match_indices(&query) {
                    if matches.len() >= limit.clamp(1, 100) as usize {
                        truncated = true;
                        break 'files;
                    }
                    let prefix = source[..offset]
                        .chars()
                        .rev()
                        .take(32)
                        .collect::<Vec<_>>()
                        .into_iter()
                        .rev()
                        .collect::<String>();
                    let suffix: String = source[offset + query.len()..].chars().take(32).collect();
                    matches.push(json!({"line":source[..offset].bytes().filter(|b| *b == b'\n').count()+1,"anchor":{"path":name,"exact":query,"prefix":prefix,"suffix":suffix,"position":source[..offset].encode_utf16().count()}}));
                }
            }
            json!({"matches":matches,"truncated":truncated})
        }
        InspectCommand::Thread { id } => {
            json!({"thread":snapshot.comments.iter().find(|item| item["id"].as_str() == Some(&id)).ok_or("unknown comment")?})
        }
        InspectCommand::Bibliography { path } => {
            if !path.is_empty() {
                selected(snapshot, &path)?;
            }
            json!({"bibliography":files.iter().filter(|(name,_)| if path.is_empty() { name.ends_with(".bib") } else { *name == &path }).map(|(name,source)|json!({"path":name,"source":source})).collect::<Vec<_>>()})
        }
        InspectCommand::Changes { .. } => unreachable!("history is read by inspect"),
    };
    let mut result = content;
    result["revision"] = json!(snapshot.sha);
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn snapshot() -> Snapshot {
        Snapshot {
            main: "main.md".into(),
            sha: "revision".into(),
            texts: json!({"main.md":"# Intro\n😀 repeated repeated\n```\n# Not a heading\n```\n## End","refs.bib":"@book{key,title={Title}}"}),
            comments: vec![json!({"id":"thread","body":"Why?","replies":[{"body":"Because"}]})],
            ..Default::default()
        }
    }
    #[test]
    fn search_preserves_occurrence_and_utf16_anchor() {
        let value = inspect_snapshot(
            &snapshot(),
            InspectCommand::Search {
                query: "repeated".into(),
                path: "".into(),
                limit: 1,
            },
        )
        .unwrap();
        assert_eq!(value["matches"][0]["anchor"]["position"], 11);
        assert_eq!(value["matches"][0]["line"], 2);
        assert_eq!(value["truncated"], true);
        assert_eq!(value["revision"], "revision");
    }
    #[test]
    fn sections_and_headings_exclude_unrelated_source() {
        let value = inspect_snapshot(
            &snapshot(),
            InspectCommand::Headings {
                path: "main.md".into(),
            },
        )
        .unwrap();
        assert_eq!(value["headings"].as_array().unwrap().len(), 2);
        let value = inspect_snapshot(
            &snapshot(),
            InspectCommand::Section {
                path: "".into(),
                line: 2,
                end_line: Some(2),
            },
        )
        .unwrap();
        assert_eq!(value["source"], "😀 repeated repeated");
        assert!(inspect_snapshot(
            &snapshot(),
            InspectCommand::Section {
                path: "".into(),
                line: 0,
                end_line: None
            }
        )
        .is_err());
    }
    #[test]
    fn thread_and_bibliography_are_focused() {
        let value = inspect_snapshot(
            &snapshot(),
            InspectCommand::Thread {
                id: "thread".into(),
            },
        )
        .unwrap();
        assert_eq!(value["thread"]["replies"][0]["body"], "Because");
        let value = inspect_snapshot(
            &snapshot(),
            InspectCommand::Bibliography { path: "".into() },
        )
        .unwrap();
        assert_eq!(value["bibliography"].as_array().unwrap().len(), 1);
    }
}
