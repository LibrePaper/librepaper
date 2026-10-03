//! What the shell crate serves and what the server is handed: the file type,
//! the content types, and the renderer list. Here, not in `librepaper-shell`,
//! so that a rebuilt `web/dist` recompiles the shell crate and not the server.

pub fn content_type(name: &str) -> &'static str {
    match name.rsplit('.').next() {
        Some("html") => "text/html; charset=utf-8",
        Some("css") => "text/css; charset=utf-8",
        // `.mjs` as well as `.js`: pdf.js ships its worker under that
        // extension and the bundler emits it under that extension, and a
        // module worker served as `application/octet-stream` is refused by
        // the browser before it runs a line.
        Some("js") | Some("mjs") => "text/javascript; charset=utf-8",
        Some("json") => "application/json; charset=utf-8",
        Some("map") => "application/json; charset=utf-8",
        Some("png") => "image/png",
        Some("jpg") | Some("jpeg") => "image/jpeg",
        Some("svg") => "image/svg+xml",
        Some("wasm") => "application/wasm",
        Some("woff2") => "font/woff2",
        Some("txt") | Some("md") => "text/plain; charset=utf-8",
        _ => "application/octet-stream",
    }
}

#[derive(Clone, Debug)]
pub struct ShellFile {
    pub kind: &'static str,
    pub body: axum::body::Bytes,
    /// A precompressed HTTP representation of `body`, when the build supplied
    /// one, so serving a bundle costs no compression work per request.
    pub brotli: Option<axum::body::Bytes>,
    /// A file whose bytes never change under this name, so it can be cached
    /// for a year rather than five minutes. Everything the bundler names for
    /// its own contents is one.
    pub immutable: bool,
}

/// The source formats this process can render in a reader, and so offer an
/// editor for. Markdown and typst are rendered by modules the browser loads
/// from the asset mirror; HTML needs no module at all, its renderer being the
/// identity.
pub fn renderers() -> Vec<String> {
    vec![
        "markdown".to_string(),
        "quarto".to_string(),
        "html".to_string(),
        "typst".to_string(),
    ]
}
