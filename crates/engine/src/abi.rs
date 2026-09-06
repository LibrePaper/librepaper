//! The WebAssembly interface: plain exports rather than wasm-bindgen.
//!
//! Exported functions take and return offsets into this module's own memory,
//! so the build needs nothing but cargo and a wasm32 target -- no bindgen CLI
//! whose version has to match the crate's -- and the loader on the other side
//! is a few lines of JavaScript. See editor.js in the shell.
//!
//! One convention: the caller allocates, writes UTF-8 into this module's
//! memory, calls `compile` or `title_of`, and reads the result back out of it.
//! Both return the length; `output_ptr` says where it starts and `ok` whether
//! there is a document.
//!
//! A compile leaves a second result beside the first: `diagnostics()` is the
//! length of the JSON list of what the compiler had to say and
//! `diagnostics_ptr()` where it starts. Two results rather than one envelope,
//! because the page is a megabyte and the list is a hundred bytes, and
//! wrapping the first in JSON to carry the second would be an encode and a
//! decode of the wrong thing on every keystroke.
//!
//! Which renderer `compile` is depends on which feature this module was built
//! with, so markdown.wasm and typst.wasm share a loader and differ only in
//! what they do with a source.

use crate::diagnostic::{Compiled, RenderedDocument};

/// Where the last result lives until the next call replaces it.
static mut OUTPUT: Option<Vec<u8>> = None;
static mut OK: bool = false;
/// The format of `OUTPUT`: 0 = no output, 1 = HTML text, 2 = PDF bytes.
static mut OUTPUT_KIND: u32 = 0;
/// What the last compile had to say, as JSON, beside the page.
static mut DIAGNOSTICS: Option<Vec<u8>> = None;
/// And the same, kept as it was, so the page shown where a document would be
/// can be built from it without the host sending it back.
static mut SAID: Option<Vec<crate::diagnostic::Diagnostic>> = None;
#[cfg(feature = "typst")]
static mut TODAY: Option<crate::typst::Today> = None;
/// The files the host has handed this module, which is the whole of what a
/// document may read. Filled from the document's own directory on the command
/// line, and in a browser from the shared document: a document is a directory,
/// and every text and figure in it is put here before a compile.
static mut FILES: Option<Vec<(String, Vec<u8>)>> = None;
/// What the main file is called, which is not decoration: a sibling resolves
/// relative to it, so a main file at `chapters/paper.typ` reaches `lib.typ`
/// beside it and not one at the root, and a diagnostic in an imported file is
/// named against it. Unset means `main.typ`, which is what a document with one
/// file has always been called here.
static mut MAIN: Option<String> = None;
/// Where each figure is, for markdown. Empty for typst, which needs no URLs,
/// and empty on the command line, where a markdown document's images are
/// relative paths that the page keeps as written.
static mut ASSET_URLS: Option<Vec<(String, String)>> = None;

/// Reserves `len` bytes for the caller to write a source into.
#[no_mangle]
pub extern "C" fn alloc(len: usize) -> *mut u8 {
    let mut buffer = Vec::with_capacity(len);
    let pointer = buffer.as_mut_ptr();
    std::mem::forget(buffer);
    pointer
}

/// Releases what `alloc` reserved. The caller frees the source it wrote; the
/// output belongs to this module and is replaced on the next call.
///
/// # Safety
/// `pointer` and `len` must be exactly what a previous `alloc` returned.
#[no_mangle]
pub unsafe extern "C" fn dealloc(pointer: *mut u8, len: usize) {
    drop(Vec::from_raw_parts(pointer, 0, len));
}

unsafe fn text_at<'a>(pointer: *const u8, len: usize) -> &'a str {
    if pointer.is_null() || len == 0 {
        return "";
    }
    std::str::from_utf8(std::slice::from_raw_parts(pointer, len)).unwrap_or("")
}

unsafe fn answer(result: Result<String, String>) -> usize {
    let (ok, text) = match result {
        Ok(html) => (true, html),
        Err(message) => (false, message),
    };
    let bytes = text.into_bytes();
    let length = bytes.len();
    OUTPUT = Some(bytes);
    OK = ok;
    OUTPUT_KIND = u32::from(ok);
    length
}

/// A compile's two results: the page where `output_ptr` looks for it, and the
/// list where `diagnostics_ptr` does. A document that did not compile leaves
/// no page and is not an error of the caller's; the list says what happened.
unsafe fn answer_compiled(compiled: Compiled) -> usize {
    DIAGNOSTICS = Some(compiled.diagnostics_json().into_bytes());
    SAID = Some(compiled.diagnostics.clone());
    let output = compiled.output;
    let (ok, kind, bytes) = match output {
        Some(RenderedDocument::Html(html)) => (true, 1, html.into_bytes()),
        Some(RenderedDocument::Pdf(pdf)) => (true, 2, pdf),
        None => (false, 0, Vec::new()),
    };
    let length = bytes.len();
    OUTPUT = Some(bytes);
    OK = ok;
    OUTPUT_KIND = kind;
    length
}

/// Renders the `source_len` bytes of UTF-8 at `source` into the page a save
/// would store, titled with the `title_len` bytes at `title`, and returns the
/// length of the result. Read it from `output_ptr()`, and ask `ok()` whether it
/// is a document or the reason it is not.
///
/// # Safety
/// The pointers and lengths must describe UTF-8 written into this module's
/// memory.
#[no_mangle]
pub unsafe extern "C" fn compile(
    source: *const u8,
    source_len: usize,
    title: *const u8,
    title_len: usize,
) -> usize {
    let source = text_at(source, source_len);
    let title = text_at(title, title_len);
    answer_compiled(render(source, title))
}

#[cfg(feature = "typst")]
fn render(source: &str, title: &str) -> Compiled {
    let today = unsafe { *std::ptr::addr_of!(TODAY) };
    let name = unsafe { (*std::ptr::addr_of!(MAIN)).clone() }.unwrap_or_default();
    crate::typst::render(source, title, &name, &from_host, today)
}

/// Reads a file the host put in the map, and nothing else: there is no
/// directory in a browser, and a document reaches only what it was given.
#[cfg(feature = "typst")]
fn from_host(path: &std::path::Path) -> Option<Vec<u8>> {
    let wanted = path.to_string_lossy();
    unsafe {
        (*std::ptr::addr_of!(FILES))
            .as_ref()?
            .iter()
            .find(|(name, _)| name.as_str() == wanted)
            .map(|(_, bytes)| bytes.clone())
    }
}

#[cfg(all(feature = "markdown", not(feature = "typst")))]
fn render(source: &str, title: &str) -> Compiled {
    crate::markdown::compile_with(source, title, &asset_url)
}

/// Where the host put a figure, for markdown's image rewriting.
#[cfg(all(feature = "markdown", not(feature = "typst")))]
fn asset_url(path: &str) -> Option<String> {
    unsafe {
        (*std::ptr::addr_of!(ASSET_URLS))
            .as_ref()?
            .iter()
            .find(|(name, _)| name.as_str() == path)
            .map(|(_, url)| url.clone())
    }
}

/// Puts a file where the next compile can read it, under the path a document
/// would import it by. This is what `#import` and `#bibliography` need in a
/// host that has no directory; the browser's map stays empty until several
/// files can travel with one source.
///
/// # Safety
/// The pointers and lengths must describe memory written into this module.
#[no_mangle]
pub unsafe extern "C" fn add_file(
    path: *const u8,
    path_len: usize,
    body: *const u8,
    body_len: usize,
) {
    let name = text_at(path, path_len).to_string();
    let bytes = if body.is_null() || body_len == 0 {
        Vec::new()
    } else {
        std::slice::from_raw_parts(body, body_len).to_vec()
    };
    let files = (*std::ptr::addr_of_mut!(FILES)).get_or_insert_with(Vec::new);
    match files.iter_mut().find(|(known, _)| *known == name) {
        Some(slot) => slot.1 = bytes,
        None => files.push((name, bytes)),
    }
}

/// Empties the map, which a host does before every document it compiles: the
/// files of the last one are not the files of this one. The main file's name
/// goes with them, since it named a document that is no longer being compiled.
#[no_mangle]
pub extern "C" fn clear_files() {
    unsafe {
        FILES = None;
        MAIN = None;
        ASSET_URLS = None;
    }
}

/// Where a figure the document names actually is, for the renderer that needs
/// a URL rather than bytes. Markdown names its images by path and the page it
/// produces is HTML a browser will fetch from; typst needs none of this, since
/// it reads a figure through the file map and writes it into the page itself.
///
/// Set with the files, before the compile, and cleared with them.
///
/// # Safety
/// The pointers and lengths must describe UTF-8 written into this module.
#[no_mangle]
pub unsafe extern "C" fn set_asset_url(
    path: *const u8,
    path_len: usize,
    url: *const u8,
    url_len: usize,
) {
    let name = text_at(path, path_len).to_string();
    let where_it_is = text_at(url, url_len).to_string();
    let urls = (*std::ptr::addr_of_mut!(ASSET_URLS)).get_or_insert_with(Vec::new);
    match urls.iter_mut().find(|(known, _)| *known == name) {
        Some(slot) => slot.1 = where_it_is,
        None => urls.push((name, where_it_is)),
    }
}

/// Names the main file, so that what it imports resolves relative to it and a
/// diagnostic in another file is named against it. The host sets this with the
/// files, before the compile; a host that does not gets `main.typ`, which is
/// what a one-file document has always been called here.
///
/// # Safety
/// The pointer and length must describe UTF-8 written into this module.
#[no_mangle]
pub unsafe extern "C" fn set_main(path: *const u8, len: usize) {
    let name = text_at(path, len).to_string();
    MAIN = if name.is_empty() { None } else { Some(name) };
}

/// The page to show where a document would be when the last compile produced
/// none: what it said, dressed as a document rather than as a crash. Built
/// from the diagnostics that compile left, so the host does not send them
/// back. Returned the way `compile` returns its page, which it therefore
/// replaces: ask for it after reading the page, not before.
///
/// # Safety
/// `title` and `len` must describe UTF-8 written into this module's memory.
#[no_mangle]
pub unsafe extern "C" fn failure_page(title: *const u8, len: usize) -> usize {
    let title = text_at(title, len);
    let said = (*std::ptr::addr_of!(SAID)).clone().unwrap_or_default();
    answer(Ok(crate::diagnostic::diagnostics_page(&said, title)))
}

/// The document's first heading, which names a document that was never given
/// a title of its own. Returned the same way `compile` returns its page.
///
/// # Safety
/// `source` and `len` must describe UTF-8 written into this module's memory.
#[no_mangle]
pub unsafe extern "C" fn title_of(source: *const u8, len: usize) -> usize {
    answer(Ok(heading(text_at(source, len))))
}

#[cfg(feature = "typst")]
fn heading(source: &str) -> String {
    crate::typst::title_of(source)
}

#[cfg(all(feature = "markdown", not(feature = "typst")))]
fn heading(source: &str) -> String {
    crate::markdown::title_of(source)
}

/// Tells the compiler what day it is, for `datetime.today()`: the engine has
/// no clock, so the browser hands it one. Ignored by the markdown module.
#[no_mangle]
pub extern "C" fn set_today(year: i32, month: u32, day: u32) {
    #[cfg(feature = "typst")]
    unsafe {
        TODAY = Some(crate::typst::Today {
            year,
            month: month as u8,
            day: day as u8,
        });
    }
    #[cfg(not(feature = "typst"))]
    let _ = (year, month, day);
}

/// Where the last result starts.
#[no_mangle]
pub extern "C" fn output_ptr() -> *const u8 {
    unsafe {
        match &*std::ptr::addr_of!(OUTPUT) {
            Some(bytes) => bytes.as_ptr(),
            None => std::ptr::null(),
        }
    }
}

/// Whether the last result is a document (1) or nothing (0).
#[no_mangle]
pub extern "C" fn ok() -> u32 {
    unsafe { u32::from(*std::ptr::addr_of!(OK)) }
}

/// Which format `output_ptr()` points to: 0 means no output, 1 HTML, and 2
/// PDF. The value is read after `compile` and before any call that replaces
/// the output buffer.
#[no_mangle]
pub extern "C" fn output_kind() -> u32 {
    unsafe { OUTPUT_KIND }
}

/// The length of what the last compile had to say, as a JSON list. Empty --
/// `[]`, two bytes -- for markdown, which cannot fail, and for a typst compile
/// that had nothing to report.
#[no_mangle]
pub extern "C" fn diagnostics() -> usize {
    unsafe {
        match &*std::ptr::addr_of!(DIAGNOSTICS) {
            Some(bytes) => bytes.len(),
            None => 0,
        }
    }
}

/// Where that list starts.
#[no_mangle]
pub extern "C" fn diagnostics_ptr() -> *const u8 {
    unsafe {
        match &*std::ptr::addr_of!(DIAGNOSTICS) {
            Some(bytes) => bytes.as_ptr(),
            None => std::ptr::null(),
        }
    }
}
