//! Pandoc citations for Markdown and Quarto.

use std::collections::{BTreeMap, BTreeSet};

use comrak::nodes::{Ast, AstNode, NodeValue};
use comrak::{format_html, parse_document, Arena};
use hayagriva::archive::ArchivedStyle;
use hayagriva::citationberg::{IndependentStyle, Style};
use hayagriva::{
    BibliographyDriver, BibliographyRequest, CitationItem, CitationRequest, CitePurpose,
    LocatorPayload, SpecificLocator,
};

use crate::diagnostic::{Compiled, Diagnostic, RenderedDocument, Severity};
use crate::markdown::{options, rewrite_images, Resolve};
use crate::page;

pub type Library = crate::bib::Library;

/// Hayagriva's parsed representation, reusable while only the Markdown prose
/// changes.
pub struct PreparedLibrary {
    hay: hayagriva::Library,
    locales: Vec<hayagriva::citationberg::Locale>,
    diagnostics: Vec<Diagnostic>,
}

pub fn prepare(library: &Library) -> PreparedLibrary {
    let (hay, diagnostics) = to_hayagriva(library);
    PreparedLibrary {
        hay,
        diagnostics,
        locales: hayagriva::archive::locales(),
    }
}

pub fn compile(
    main: &str,
    source: &str,
    title: &str,
    texts: &BTreeMap<String, String>,
    assets: Resolve<'_>,
) -> Compiled {
    let library = crate::bib::library(main, "markdown", source, texts);
    compile_with_library(main, source, title, &library, assets)
}

pub fn compile_with_library(
    main: &str,
    source: &str,
    title: &str,
    library: &Library,
    assets: Resolve<'_>,
) -> Compiled {
    let prepared = prepare(library);
    compile_prepared(main, source, title, &prepared, assets)
}

pub fn compile_prepared(
    main: &str,
    source: &str,
    title: &str,
    prepared: &PreparedLibrary,
    assets: Resolve<'_>,
) -> Compiled {
    let hay = &prepared.hay;
    let mut diagnostics = prepared.diagnostics.clone();
    let locales = &prepared.locales;
    let style_name = front_matter_value(source, "csl")
        .or_else(|| front_matter_value(source, "bibliography-style"));
    let (style, warning) = style_for(style_name.as_deref());
    if let Some(warning) = warning {
        diagnostics.push(warning);
    }
    let arena = Arena::new();
    let root = parse_document(&arena, source, &options());
    let occurrences: Vec<_> = root
        .descendants()
        .filter_map(|node| {
            if node.ancestors().skip(1).any(|ancestor| {
                matches!(
                    ancestor.data().value,
                    NodeValue::Link(_) | NodeValue::Image(_)
                )
            }) {
                return None;
            }
            let text = match &node.data().value {
                NodeValue::Text(text) => text.to_string(),
                _ => return None,
            };
            let matches: Vec<CitationMatch> = find_matches(&text)
                .into_iter()
                .filter(|matched| !is_escaped(source, node, matched))
                .collect();
            (!matches.is_empty()).then_some(Occurrence {
                node,
                text,
                matches,
            })
        })
        .collect();

    let mut driver = BibliographyDriver::new();
    let mut indices: Vec<Vec<Option<usize>>> = Vec::with_capacity(occurrences.len());
    let mut cited = BTreeSet::new();
    let mut citation_count = 0;
    for occurrence in &occurrences {
        let mut local = Vec::with_capacity(occurrence.matches.len());
        for matched in &occurrence.matches {
            let known: Vec<_> = matched
                .parts
                .iter()
                .filter_map(|part| {
                    let entry = hay.get(&part.key)?;
                    cited.insert(part.key.clone());
                    let locator = part.locator.as_ref().map(|locator| {
                        SpecificLocator(locator.kind, LocatorPayload::Str(&locator.value))
                    });
                    Some(CitationItem::new(
                        entry,
                        locator,
                        None,
                        false,
                        part.purpose.filter(|purpose| *purpose != CitePurpose::Year),
                    ))
                })
                .collect();
            if known.is_empty() {
                local.push(None);
            } else {
                let index = citation_count;
                citation_count += 1;
                local.push(Some(index));
                driver.citation(CitationRequest::from_items(known, &style, locales));
            }
            for part in &matched.parts {
                if hay.get(&part.key).is_none() {
                    let local = matched.start + if matched.bracketed { 1 } else { 0 } + part.start;
                    let offset = source_offset(source, occurrence.node, local, &part.key);
                    diagnostics.push(missing_diagnostic(&part.key, main, source, offset));
                }
            }
        }
        indices.push(local);
    }
    let rendered = driver.finish(BibliographyRequest::new(&style, None, locales));
    for (occurrence, local) in occurrences.iter().zip(indices) {
        let mut replacement = String::new();
        let mut cursor = 0;
        for (matched, index) in occurrence.matches.iter().zip(local) {
            replacement.push_str(&page::escape(&occurrence.text[cursor..matched.start]));
            let unknown: Vec<String> = matched
                .parts
                .iter()
                .filter(|part| hay.get(&part.key).is_none())
                .map(|part| format!("@{}", part.key))
                .collect();
            let local_match = CitationMatch {
                start: 0,
                end: matched.end - matched.start,
                bracketed: matched.bracketed,
                parts: matched.parts.clone(),
            };
            replacement.push_str(&replace_match(
                &occurrence.text[matched.start..matched.end],
                &local_match,
                index.and_then(|i| rendered.citations.get(i)),
                &unknown,
                hay,
            ));
            cursor = matched.end;
        }
        replacement.push_str(&page::escape(&occurrence.text[cursor..]));
        replace_node(&arena, occurrence.node, &replacement);
    }
    if !cited.is_empty() {
        let section = reference_section(&rendered, &cited, !has_references_heading(root));
        if !section.is_empty() {
            let node = arena.alloc(NodeValue::Raw(section).into());
            if let Some(heading) = root.descendants().find(|node| is_references_heading(node)) {
                heading.insert_after(node);
            } else {
                root.append(node);
            }
        }
    }
    let mut body = String::new();
    format_html(root, &options(), &mut body).expect("String writes cannot fail");
    let body = rewrite_images(&body, assets);
    Compiled {
        output: Some(RenderedDocument::Html(page::page(title, "", &body))),
        diagnostics,
    }
}

fn is_escaped(source: &str, node: &AstNode<'_>, matched: &CitationMatch) -> bool {
    let base = source
        .lines()
        .take(node.data().sourcepos.start.line.saturating_sub(1))
        .map(|line| line.len() + 1)
        .sum::<usize>()
        + node.data().sourcepos.start.column.saturating_sub(1);
    let opening = base + matched.start;
    if source.as_bytes().get(opening.saturating_sub(1)) == Some(&b'\\') {
        return true;
    }
    matched.parts.iter().any(|part| {
        let at = base + matched.start + if matched.bracketed { 1 } else { 0 } + part.start;
        source.as_bytes().get(at.saturating_sub(1)) == Some(&b'\\')
            || source.as_bytes().get(at) == Some(&b'\\')
    })
}

struct Occurrence<'a> {
    node: &'a AstNode<'a>,
    text: String,
    matches: Vec<CitationMatch>,
}
#[derive(Clone)]
struct CitationMatch {
    start: usize,
    end: usize,
    bracketed: bool,
    parts: Vec<CitationPart>,
}
#[derive(Clone)]
struct CitationPart {
    key: String,
    purpose: Option<CitePurpose>,
    start: usize,
    locator_end: usize,
    locator: Option<LocatorSpec>,
    prefix: String,
    suffix: String,
}

#[derive(Clone)]
struct LocatorSpec {
    kind: hayagriva::citationberg::taxonomy::Locator,
    value: String,
}

fn find_matches(text: &str) -> Vec<CitationMatch> {
    let mut result = Vec::new();
    let mut cursor = 0;
    while cursor < text.len() {
        let Some(relative) = text[cursor..].find('@') else {
            break;
        };
        let at = cursor + relative;
        if at > 0
            && text[..at].chars().last().is_some_and(|c| {
                c.is_alphanumeric()
                    || matches!(c, '/' | ':' | '.')
                    || (c == '-' && !in_bracket(text, at))
            })
        {
            cursor = at + 1;
            continue;
        }
        let (open, delimiter) = opening_delimiter(text, at);
        let bracket = open
            .filter(|open| text[open + 1..at].find(delimiter).is_none())
            .and_then(|open| text[at..].find(delimiter).map(|close| (open, close)));
        let (start, end, inner, bracketed) = if let Some((open, close)) = bracket {
            (open, at + close + 1, &text[open + 1..at + close], true)
        } else {
            let end = at + 1 + key_end(&text[at + 1..]);
            (at, end, &text[at..end], false)
        };
        let mut parts = parse_parts(inner, bracketed);
        if !bracketed {
            parts.truncate(1);
        }
        if parts.is_empty() {
            cursor = at + 1;
        } else {
            result.push(CitationMatch {
                start,
                end,
                bracketed,
                parts,
            });
            cursor = end;
        }
    }
    result
}

fn parse_parts(text: &str, bracketed: bool) -> Vec<CitationPart> {
    let mut result = Vec::new();
    let mut cursor = 0;
    while let Some(relative) = text[cursor..].find('@') {
        let at = cursor + relative;
        if at > 0
            && text[..at]
                .chars()
                .last()
                .is_some_and(|c| c.is_alphanumeric() || matches!(c, '/' | ':' | '.'))
        {
            cursor = at + 1;
            continue;
        }
        let len = key_end(&text[at + 1..]);
        if len == 0 {
            cursor = at + 1;
            continue;
        }
        let suppress = at > 0 && text.as_bytes()[at - 1] == b'-';
        let locator = parse_locator(&text[at + 1 + len..]);
        result.push(CitationPart {
            key: text[at + 1..at + 1 + len].to_string(),
            purpose: suppress
                .then_some(CitePurpose::Year)
                .or_else(|| (!bracketed && at == 0).then_some(CitePurpose::Prose)),
            start: at,
            locator_end: at + len + 1 + locator.as_ref().map_or(0, |l| l.0),
            locator: locator.map(|(_, l)| l),
            prefix: String::new(),
            suffix: String::new(),
        });
        cursor = at + len + 1;
    }
    if bracketed {
        for part in &mut result {
            let prefix_start = text[..part.start].rfind(';').map_or(0, |at| at + 1);
            let suffix_end = text[part.locator_end..]
                .find(';')
                .map_or(text.len(), |at| part.locator_end + at);
            let prefix = text[prefix_start..part.start].trim_start();
            let prefix = if part.purpose == Some(CitePurpose::Year) {
                prefix.trim_end_matches('-')
            } else {
                prefix
            };
            let suffix = text[part.locator_end..suffix_end].to_string();
            part.prefix = prefix.to_string();
            part.suffix = suffix;
        }
    }
    result
}

fn in_bracket(text: &str, at: usize) -> bool {
    let (open, delimiter) = opening_delimiter(text, at);
    open.is_some_and(|open| text[open + 1..at].find(delimiter).is_none())
}

fn opening_delimiter(text: &str, at: usize) -> (Option<usize>, char) {
    let square = text[..at].rfind('[');
    let paren = text[..at].rfind('(');
    match (square, paren) {
        (Some(square), Some(paren)) if paren > square => (Some(paren), ')'),
        (Some(square), _) => (Some(square), ']'),
        (None, Some(paren)) => (Some(paren), ')'),
        (None, None) => (None, ']'),
    }
}

/// Parse Pandoc's conventional `, pp. 33-35` locator suffix.
fn parse_locator(text: &str) -> Option<(usize, LocatorSpec)> {
    let limit = text.find(['@', ';']).unwrap_or(text.len());
    let candidate = &text[..limit];
    let trimmed = candidate.trim_end();
    let (comma, body) = trimmed
        .strip_prefix(',')
        .map_or((false, trimmed), |body| (true, body.trim_start()));
    let mut words = body.splitn(2, char::is_whitespace);
    let label = words.next()?.trim_end_matches('.').to_ascii_lowercase();
    let value = words.next()?.trim();
    let kind = match label.as_str() {
        "p" | "pp" | "page" | "pages" => hayagriva::citationberg::taxonomy::Locator::Page,
        "chap" | "chapter" => hayagriva::citationberg::taxonomy::Locator::Chapter,
        "sec" | "section" => hayagriva::citationberg::taxonomy::Locator::Section,
        "fig" | "figure" => hayagriva::citationberg::taxonomy::Locator::Figure,
        "para" | "paragraph" => hayagriva::citationberg::taxonomy::Locator::Paragraph,
        "vol" | "volume" => hayagriva::citationberg::taxonomy::Locator::Volume,
        _ => return None,
    };
    let value_start = body.find(value)?;
    let consumed = if comma {
        candidate.len() - body.len() + value_start + value.len()
    } else {
        value_start + value.len()
    };
    Some((
        consumed,
        LocatorSpec {
            kind,
            value: value.to_string(),
        },
    ))
}

fn key_end(text: &str) -> usize {
    let mut end = 0;
    for (index, c) in text.char_indices() {
        if c.is_alphanumeric() || matches!(c, '_' | ':' | '-' | '.' | '+') {
            end = index + c.len_utf8();
        } else {
            break;
        }
    }
    if text[..end].ends_with('.') {
        end -= 1;
    }
    end
}

fn replace_match(
    original: &str,
    matched: &CitationMatch,
    rendered: Option<&hayagriva::RenderedCitation>,
    unknown: &[String],
    hay: &hayagriva::Library,
) -> String {
    let Some(rendered) = rendered else {
        return page::escape(original);
    };
    let mut html = String::new();
    let known: Vec<&CitationPart> = matched
        .parts
        .iter()
        .filter(|part| hay.get(&part.key).is_some())
        .collect();
    if !matched.bracketed || known.is_empty() {
        safe_children(&rendered.citation, &mut html);
    } else {
        for child in &rendered.citation.0 {
            if let Some(index) = entry_index_of(child).filter(|index| *index < known.len()) {
                let part = known[index];
                html.push_str(&page::escape(&part.prefix));
                if part.purpose == Some(CitePurpose::Year) {
                    let mut content = child.clone();
                    suppress_author(&mut content);
                    safe_child(&content, &mut html);
                } else {
                    safe_child(child, &mut html);
                }
                html.push_str(&page::escape(&part.suffix));
            } else {
                safe_child(child, &mut html);
            }
        }
    }
    for key in unknown {
        if !html.contains(key) {
            html.push_str("; ");
            html.push_str(&page::escape(key));
        }
    }
    format!("<span class=\"citation\">{html}</span>")
}

// Keep the style's date, locator and wrapper while suppressing its names.
// Numeric styles have no names to remove, so their citation numbers remain.
fn suppress_author(child: &mut hayagriva::ElemChild) -> bool {
    let hayagriva::ElemChild::Elem(element) = child else {
        return false;
    };
    if matches!(element.meta, Some(hayagriva::ElemMeta::Names(_))) {
        element.children.0.clear();
        return true;
    }
    let mut removed = false;
    let mut trim_delimiter = false;
    for child in &mut element.children.0 {
        if suppress_author(child) {
            removed = true;
            trim_delimiter = true;
        } else if trim_delimiter {
            if let hayagriva::ElemChild::Text(text) = child {
                text.text = text
                    .text
                    .trim_start_matches(|c: char| c.is_whitespace() || c == ',' || c == ';')
                    .to_string();
                if !text.text.is_empty() {
                    trim_delimiter = false;
                }
            } else {
                trim_delimiter = false;
            }
        }
    }
    removed
}
fn source_offset(source: &str, node: &AstNode<'_>, local: usize, key: &str) -> usize {
    let data = node.data();
    let start = data.sourcepos.start;
    let line_start = source
        .split_inclusive('\n')
        .take(start.line.saturating_sub(1))
        .map(str::len)
        .sum::<usize>();
    let mut base = (line_start + start.column.saturating_sub(1)).min(source.len());
    while !source.is_char_boundary(base) {
        base -= 1;
    }
    let end = data.sourcepos.end;
    let mut limit = (source
        .split_inclusive('\n')
        .take(end.line.saturating_sub(1))
        .map(str::len)
        .sum::<usize>()
        + end.column)
        .min(source.len());
    while !source.is_char_boundary(limit) {
        limit += 1;
    }
    let token = format!("@{key}");
    // Comrak has decoded escapes and entities. Locate the same token in its
    // original source span rather than adding decoded byte offsets to it.
    let preceding = match &data.value {
        NodeValue::Text(text) => text[..local.min(text.len())].matches(&token).count(),
        _ => 0,
    };
    if let Some((at, _)) = source[base..limit.max(base)]
        .match_indices(&token)
        .nth(preceding)
    {
        return base + at;
    }
    let mut at = (base + local).min(source.len());
    while !source.is_char_boundary(at) {
        at -= 1;
    }
    at
}

fn entry_index_of(child: &hayagriva::ElemChild) -> Option<usize> {
    match child {
        hayagriva::ElemChild::Elem(element) => match element.meta {
            Some(hayagriva::ElemMeta::Entry(index)) => Some(index),
            _ => None,
        },
        _ => None,
    }
}

fn replace_node<'a>(arena: &'a Arena<'a>, node: &'a AstNode<'a>, replacement: &str) {
    let sourcepos = node.data().sourcepos;
    let new_node = arena.alloc(AstNode::from(Ast::new_with_sourcepos(
        NodeValue::Raw(replacement.to_string()),
        sourcepos,
    )));
    node.insert_before(new_node);
    node.detach();
}

fn style_for(name: Option<&str>) -> (IndependentStyle, Option<Diagnostic>) {
    let requested = name.unwrap_or("apa");
    let archived = ArchivedStyle::by_name(requested)
        .or_else(|| ArchivedStyle::by_name("apa"))
        .expect("bundled APA style");
    let style = match archived.get() {
        Style::Independent(style) => style,
        Style::Dependent(_) => unreachable!("bundled styles are independent"),
    };
    let warning = (name.is_some() && ArchivedStyle::by_name(requested).is_none()).then(|| {
        Diagnostic::spanless(
            Severity::Warning,
            format!("unknown citation style {requested}; using APA"),
        )
    });
    (style, warning)
}

fn reference_section(
    rendered: &hayagriva::Rendered,
    cited: &BTreeSet<String>,
    heading: bool,
) -> String {
    let Some(bibliography) = &rendered.bibliography else {
        return String::new();
    };
    let mut html = String::from("<section id=\"references\" class=\"references\">");
    if heading {
        html.push_str("<h2>References</h2>");
    }
    for item in &bibliography.items {
        if !cited.contains(&item.key) {
            continue;
        }
        html.push_str("<div class=\"reference\" id=\"ref-");
        html.push_str(&page::escape(&item.key));
        html.push_str("\">");
        if let Some(first) = &item.first_field {
            safe_child(first, &mut html);
            html.push(' ');
        }
        safe_children(&item.content, &mut html);
        html.push_str("</div>");
    }
    html.push_str("</section>");
    html
}

fn safe_children(children: &hayagriva::ElemChildren, html: &mut String) {
    for child in &children.0 {
        safe_child(child, html);
    }
}

fn safe_child(child: &hayagriva::ElemChild, html: &mut String) {
    match child {
        hayagriva::ElemChild::Text(text) => {
            let formatting = text.formatting;
            if formatting.font_style == hayagriva::citationberg::FontStyle::Italic {
                html.push_str("<i>");
            }
            if formatting.font_weight == hayagriva::citationberg::FontWeight::Bold {
                html.push_str("<strong>");
            }
            html.push_str(&page::escape(&text.text));
            if formatting.font_weight == hayagriva::citationberg::FontWeight::Bold {
                html.push_str("</strong>");
            }
            if formatting.font_style == hayagriva::citationberg::FontStyle::Italic {
                html.push_str("</i>");
            }
        }
        hayagriva::ElemChild::Elem(element) => safe_children(&element.children, html),
        hayagriva::ElemChild::Link { text, url } => {
            if url.starts_with("http://") || url.starts_with("https://") {
                html.push_str("<a href=\"");
                html.push_str(&page::escape(url));
                html.push_str("\">");
            }
            safe_child(&hayagriva::ElemChild::Text(text.clone()), html);
            if url.starts_with("http://") || url.starts_with("https://") {
                html.push_str("</a>");
            }
        }
        hayagriva::ElemChild::Markup(markup) => html.push_str(&page::escape(markup)),
        hayagriva::ElemChild::Transparent { .. } => {}
    }
}

fn to_hayagriva(library: &Library) -> (hayagriva::Library, Vec<Diagnostic>) {
    let mut output = hayagriva::Library::new();
    let mut diagnostics = library.diagnostics.clone();
    for entry in &library.entries {
        let mut bib = format!("@{}{{{},", entry.entry_type, entry.key);
        for (field, value) in &entry.raw {
            if field.starts_with("__librepaper_") || field == "crossref" {
                continue;
            }
            bib.push_str(field);
            bib.push('=');
            bib.push_str(entry.raw_biblatex.get(field).unwrap_or(value));
            bib.push(',');
        }
        bib.push('}');
        match hayagriva::io::from_biblatex_str(&bib) {
            Ok(parsed) => {
                if let Some(item) = parsed.iter().next() {
                    output.push(item);
                }
            }
            Err(_) => {
                let mut diagnostic = Diagnostic::spanless(
                    Severity::Warning,
                    format!("bibliography entry `{}` could not be formatted", entry.key),
                );
                diagnostic.file = entry.path.clone();
                diagnostic.line = entry.line;
                diagnostic.end_line = entry.line;
                diagnostics.push(diagnostic);
            }
        }
    }
    (output, diagnostics)
}

fn is_references_heading<'a>(node: &'a AstNode<'a>) -> bool {
    if !matches!(node.data().value, NodeValue::Heading(_)) {
        return false;
    }
    let mut text = String::new();
    node.collect_text_append(&mut text);
    text.trim().eq_ignore_ascii_case("references")
}

fn has_references_heading<'a>(root: &'a AstNode<'a>) -> bool {
    root.descendants().any(is_references_heading)
}

fn front_matter_value(source: &str, key: &str) -> Option<String> {
    let mut lines = source.lines();
    if lines.next()?.trim() != "---" {
        return None;
    }
    for line in lines {
        if line.trim() == "---" {
            break;
        }
        let Some((candidate, value)) = line.split_once(':') else {
            continue;
        };
        if candidate.trim() == key {
            return Some(value.trim().trim_matches(['"', '\'']).to_string());
        }
    }
    None
}

fn missing_diagnostic(key: &str, file: &str, source: &str, offset: usize) -> Diagnostic {
    let offset = offset.min(source.len());
    let line = source[..offset].bytes().filter(|b| *b == b'\n').count() + 1;
    let line_start = source[..offset].rfind('\n').map_or(0, |index| index + 1);
    let column = source[line_start..offset].encode_utf16().count() + 1;
    Diagnostic {
        severity: Severity::Warning,
        message: format!("citation key {key} is not present in the bibliography"),
        file: file.to_string(),
        line,
        column,
        end_line: line,
        end_column: column + key.encode_utf16().count() + 1,
        hints: Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn render(source: &str) -> Compiled {
        let mut texts = BTreeMap::new();
        texts.insert(
            "paper/refs.bib".into(),
            r#"@article{smith2020,
  author = {Smith, Sam},
  title = {A {Safe} Title},
  journal = {Journal},
  year = {2020},
}
@article{jones2021,
  author = {Jones, Jo},
  title = {Second Title},
  journal = {Journal},
  year = {2021},
}
@article{evil,
  author = {Danger, Dan},
  title = {<script>alert(1)</script>},
  journal = {Journal},
  year = {2022},
}
@article{brown,
  author = {Brown, Bob},
  title = {Third Title},
  journal = {Journal},
  year = {2019},
}"#
            .into(),
        );
        compile(
            "paper/main.md",
            source,
            "Paper",
            &texts,
            &crate::markdown::no_assets,
        )
    }

    #[test]
    fn renders_multiple_matches_and_narrative() {
        let result =
            render("---\nbibliography: refs.bib\n---\nAs @smith2020 shows, @jones2021 differs.\n");
        let html = result.output.unwrap().html().unwrap().to_string();
        assert!(html.contains("Smith"), "{html}");
        assert!(html.contains("Jones"), "{html}");
    }

    #[test]
    fn renders_locators_and_source_separators() {
        let result = render(
            "---\nbibliography: refs.bib\n---\n[see @smith2020, pp. 33-35; also @jones2021].\n",
        );
        let html = result.output.unwrap().html().unwrap().to_string();
        assert!(html.contains("33"), "{html}");
        assert!(html.contains("also"), "{html}");
        assert_eq!(html.matches("Smith").count(), 2, "{html}");
    }

    #[test]
    fn suppresses_author_and_keeps_missing_key_visible() {
        let result = render(
            "---\nbibliography: refs.bib\n---\n[-@smith2020] and [see @missing; also @jones2021].\n",
        );
        let html = result.output.unwrap().html().unwrap().to_string();
        assert!(html.contains("2020"), "{html}");
        assert!(html.contains("@missing"), "{html}");
        assert_eq!(
            result
                .diagnostics
                .iter()
                .filter(|d| d.message.contains("missing"))
                .count(),
            1
        );
    }

    #[test]
    fn escaped_and_linked_at_signs_are_literal() {
        let result =
            render("---\nbibliography: refs.bib\n---\n\\@smith2020 and [@smith2020](url)\n");
        let html = result.output.unwrap().html().unwrap().to_string();
        assert!(html.contains("@smith2020"), "{html}");
        assert_eq!(html.matches("citation").count(), 0, "{html}");
    }

    #[test]
    fn bibliography_text_is_html_escaped() {
        let result = render("---\nbibliography: refs.bib\n---\n[@evil]\n");
        let html = result.output.unwrap().html().unwrap().to_string();
        assert!(!html.contains("<script>"), "{html}");
        assert!(html.contains("&lt;script&gt;"), "{html}");
    }

    #[test]
    fn sorted_multi_citation_keeps_wrapper_and_item_context() {
        let result = render(
            "---\nbibliography: refs.bib\n---\n[see @smith2020, pp. 3–4; also @jones2021, p. 5; compare @brown, p. 9]\n",
        );
        let html = result.output.unwrap().html().unwrap().to_string();
        assert!(html.contains("<span class=\"citation\">("), "{html}");
        assert!(html.contains("see"), "{html}");
        assert!(html.contains("also"), "{html}");
        assert!(html.contains("compare"), "{html}");
        assert!(html.contains("3–4"), "{html}");
        assert!(html.contains("p. 5"), "{html}");
        assert!(html.contains("p. 9"), "{html}");
    }
}
