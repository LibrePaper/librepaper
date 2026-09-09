//! What a typst compile could not find, and how it is found: packages from
//! the registry into the cache, fonts from the deployment's library, and the
//! library itself as the deployment serves it.

use std::path::Path;

use serde_json::Value;
use sha2::Digest;

use super::harness::{client, get_json, new_test_server, test_server_fonts};
use crate::document::needs::{resolve, resolve_cached, untar, Cache};
use crate::document::render::{compile_from_files, Noted};
use crate::server::fonts::{families_in, Library};

const MINI_MANIFEST: &[u8] =
    b"[package]\nname = \"mini\"\nversion = \"0.1.0\"\nentrypoint = \"lib.typ\"\n";
const MINI_LIB: &[u8] =
    b"#import \"util.typ\": shout\n#let hello(name) = shout(\"hello, \" + name)\n";
const MINI_UTIL: &[u8] = b"#let shout(text) = upper(text)\n";
const PACKAGED: &str = "#import \"@preview/mini:0.1.0\": hello\n= T\n#hello(\"world\")\n";

/// A ustar archive of these files, the way the registry packs a package.
fn tar_of(files: &[(&str, &[u8])]) -> Vec<u8> {
    let mut tar = Vec::new();
    for (name, bytes) in files {
        let mut header = [0u8; 512];
        header[..name.len()].copy_from_slice(name.as_bytes());
        header[100..107].copy_from_slice(b"0000644");
        header[124..135].copy_from_slice(format!("{:011o}", bytes.len()).as_bytes());
        header[156] = b'0';
        header[257..262].copy_from_slice(b"ustar");
        header[263..265].copy_from_slice(b"00");
        header[148..156].copy_from_slice(b"        ");
        let sum: u32 = header.iter().map(|b| *b as u32).sum();
        header[148..154].copy_from_slice(format!("{sum:06o}").as_bytes());
        header[154] = 0;
        tar.extend_from_slice(&header);
        tar.extend_from_slice(bytes);
        tar.resize(tar.len() + (512 - bytes.len() % 512) % 512, 0);
    }
    tar.resize(tar.len() + 1024, 0);
    tar
}

fn gzip(bytes: &[u8]) -> Vec<u8> {
    use std::io::Write;
    let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
    encoder.write_all(bytes).unwrap();
    encoder.finish().unwrap()
}

/// A font the compiler already embeds, for a library to hold: the test is
/// about the plumbing, not the face.
fn dejavu() -> &'static [u8] {
    typst_assets::fonts()
        .find(|bytes| families_in(bytes) == vec!["dejavu sans mono".to_string()])
        .expect("typst-assets ships DejaVu Sans Mono")
}

fn cached_mini(cache: &Cache, root: &Path) {
    let dir = root.join("typst/packages/preview/mini/0.1.0");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("typst.toml"), MINI_MANIFEST).unwrap();
    std::fs::write(dir.join("lib.typ"), MINI_LIB).unwrap();
    std::fs::write(dir.join("util.typ"), MINI_UTIL).unwrap();
    assert!(cache
        .package_file(Path::new("@preview/mini/0.1.0/typst.toml"))
        .is_some());
}

#[test]
fn untar_reads_the_regular_files_and_nothing_else() {
    let tar = tar_of(&[("typst.toml", MINI_MANIFEST), ("src/lib.typ", MINI_LIB)]);
    let files = untar(&tar);
    assert_eq!(
        files,
        vec![
            ("typst.toml".to_string(), MINI_MANIFEST.to_vec()),
            ("src/lib.typ".to_string(), MINI_LIB.to_vec()),
        ]
    );
    // An archive cut short yields what it has whole, not a panic: the first
    // file is complete at 600 bytes and the second is not there yet.
    assert_eq!(untar(&tar[..600]).len(), 1);
    assert_eq!(untar(&tar[..700]).len(), 1);
    assert_eq!(untar(&[]).len(), 0);
}

#[test]
fn a_package_in_the_cache_imports_and_one_that_is_not_is_a_need() {
    let dir = tempfile::tempdir().unwrap();
    let cache = Cache::at(dir.path());
    let without = compile_from_files("main.typ", PACKAGED, "T", &[], &[], Some(&cache));
    assert!(without.compiled.output.is_none());
    assert_eq!(without.needs.packages.len(), 1, "{:?}", without.needs);
    assert_eq!(without.needs.packages[0].dir(), "@preview/mini/0.1.0");
    assert!(
        without.read.is_empty(),
        "a package is not a sibling that was read"
    );

    cached_mini(&cache, dir.path());
    let with = compile_from_files("main.typ", PACKAGED, "T", &[], &[], Some(&cache));
    assert!(
        with.compiled.output.is_some(),
        "{:?}",
        with.compiled.diagnostics
    );
    assert!(with.needs.is_empty(), "{:?}", with.needs);
    assert!(with.read.is_empty(), "{:?}", with.read);

    // The path a package is asked by cannot leave the package.
    assert!(cache
        .package_file(Path::new("@preview/mini/0.1.0/../../../../etc/passwd"))
        .is_none());
    assert!(cache
        .package_file(Path::new("@preview/../x/1.0.0/a"))
        .is_none());
}

#[test]
fn fonts_the_cache_holds_for_a_family_are_offered_on_the_second_pass() {
    let dir = tempfile::tempdir().unwrap();
    let cache = Cache::at(dir.path());
    let family_dir = dir.path().join("librepaper/fonts/stand_in");
    std::fs::create_dir_all(&family_dir).unwrap();
    std::fs::write(family_dir.join("StandIn.ttf"), dejavu()).unwrap();

    let mut passes: Vec<usize> = Vec::new();
    let noted = resolve_cached(Some(&cache), |library| {
        passes.push(library.len());
        let mut noted: Noted = compile_from_files("main.typ", "= T\n", "T", &[], library, None);
        if library.is_empty() {
            noted.needs.fonts.push("stand in".to_string());
        }
        noted
    });
    assert_eq!(
        passes,
        vec![0, 1],
        "the second pass carries the cached font"
    );
    assert!(noted.needs.is_empty());
    assert_eq!(
        cache.fonts_for(&["stand in".to_string()])[0].0,
        "stand_in/StandIn.ttf"
    );
    assert!(cache.fonts_for(&["never heard of".to_string()]).is_empty());
}

#[tokio::test]
async fn resolve_fetches_a_package_from_the_registry_and_a_font_from_the_deployment() {
    use axum::routing::get;
    let archive = gzip(&tar_of(&[
        ("typst.toml", MINI_MANIFEST),
        ("lib.typ", MINI_LIB),
        ("util.typ", MINI_UTIL),
    ]));
    let sha = hex::encode(sha2::Sha256::digest(dejavu()));
    let index = serde_json::json!({ "families": { "stand in": [format!("{sha}/StandIn.ttf")] } });
    let app = axum::Router::new()
        .route(
            "/preview/mini-0.1.0.tar.gz",
            get(move || {
                let archive = archive.clone();
                async move { archive }
            }),
        )
        .route(
            "/api/fonts/index.json",
            get(move || {
                let index = index.clone();
                async move { axum::Json(index) }
            }),
        )
        .route(
            "/api/fonts/{sha}/StandIn.ttf",
            get(|| async { dejavu().to_vec() }),
        );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let served = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });

    let dir = tempfile::tempdir().unwrap();
    let cache = Cache::at(dir.path()).with_registry(&format!("{base}/"));
    let mut passes = 0;
    let noted = resolve(Some(&cache), Some(&base), |library| {
        passes += 1;
        let mut noted = compile_from_files("main.typ", PACKAGED, "T", &[], library, Some(&cache));
        if library.is_empty() {
            noted.needs.fonts.push("stand in".to_string());
        }
        noted
    })
    .await;
    assert!(
        noted.compiled.output.is_some(),
        "{:?}",
        noted.compiled.diagnostics
    );
    assert!(noted.needs.is_empty(), "{:?}", noted.needs);
    assert_eq!(
        passes, 2,
        "one fetch round found both the package and the font"
    );
    assert!(dir
        .path()
        .join("typst/packages/preview/mini/0.1.0/util.typ")
        .is_file());
    assert!(dir
        .path()
        .join("librepaper/fonts/stand_in/StandIn.ttf")
        .is_file());

    // Fetched once: the cache answers the next document.
    let again = resolve(Some(&cache), Some(&base), |library| {
        compile_from_files("main.typ", PACKAGED, "T", &[], library, Some(&cache))
    })
    .await;
    assert!(again.compiled.output.is_some());
    served.abort();
}

#[tokio::test]
async fn resolve_gives_up_on_a_registry_that_has_no_such_package() {
    let app = axum::Router::new();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let served = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let dir = tempfile::tempdir().unwrap();
    let cache = Cache::at(dir.path()).with_registry(&format!("{base}/"));
    let mut passes = 0;
    let noted = resolve(Some(&cache), Some(&base), |library| {
        passes += 1;
        compile_from_files("main.typ", PACKAGED, "T", &[], library, Some(&cache))
    })
    .await;
    assert!(noted.compiled.output.is_none());
    assert_eq!(passes, 1, "nothing arrived, so nothing was compiled again");
    assert_eq!(noted.needs.packages.len(), 1);
    assert!(noted
        .compiled
        .errors()
        .any(|error| error.message.contains("package not found")));
    served.abort();
}

#[test]
fn a_library_is_a_directory_that_exists() {
    let missing = tempfile::tempdir().unwrap().path().join("nowhere");
    let error = Library::open(&missing.display().to_string()).expect_err("a missing directory");
    assert!(error.contains("--fonts"), "{error}");
    let empty = tempfile::tempdir().unwrap();
    let library = Library::open(&empty.path().display().to_string()).expect("an empty directory");
    assert_eq!(library.index()["families"], serde_json::json!({}));
    assert!(families_in(b"not a font").is_empty());
}

#[tokio::test]
async fn the_font_library_is_served_by_family_and_addressed_by_digest() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("mono")).unwrap();
    std::fs::write(dir.path().join("mono/DejaVuSansMono.ttf"), dejavu()).unwrap();
    std::fs::write(dir.path().join("notes.txt"), b"not a font, not served").unwrap();
    let library = Library::open(&dir.path().display().to_string()).expect("open");
    assert!(
        library.describe().contains("1 files, 1 families"),
        "{}",
        library.describe()
    );
    let server = test_server_fonts(library).await;

    let (status, config) = get_json(&server.url, "/api/config").await;
    assert_eq!(status, 200);
    assert_eq!(config["fonts"], Value::Bool(true));

    let (status, index) = get_json(&server.url, "/api/fonts/index.json").await;
    assert_eq!(status, 200);
    let files = index["families"]["dejavu sans mono"]
        .as_array()
        .expect("the family is indexed");
    let file = files[0].as_str().unwrap();
    let sha = hex::encode(sha2::Sha256::digest(dejavu()));
    assert_eq!(file, format!("{sha}/mono/DejaVuSansMono.ttf"));

    let response = client()
        .get(format!("{}/api/fonts/{file}", server.url))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    assert_eq!(response.headers()["content-type"], "font/ttf");
    assert_eq!(
        response.headers()["cache-control"],
        "public, max-age=31536000, immutable"
    );
    assert_eq!(response.bytes().await.unwrap().len(), dejavu().len());

    let head = client()
        .head(format!("{}/api/fonts/{file}", server.url))
        .send()
        .await
        .unwrap();
    assert_eq!(head.status(), 200);
    assert_eq!(head.bytes().await.unwrap().len(), 0);

    // The digest is the address: another sha is another file, and not there.
    let wrong = client()
        .get(format!(
            "{}/api/fonts/{}/mono/DejaVuSansMono.ttf",
            server.url,
            "0".repeat(64)
        ))
        .send()
        .await
        .unwrap();
    assert_eq!(wrong.status(), 404);
    let escaped = client()
        .get(format!("{}/api/fonts/{sha}/../notes.txt", server.url))
        .send()
        .await
        .unwrap();
    assert_ne!(escaped.status(), 200);

    // A deployment without a library says so, and serves nothing.
    let plain = new_test_server().await;
    let (_, config) = get_json(&plain.url, "/api/config").await;
    assert_eq!(config["fonts"], Value::Bool(false));
    let (status, _) = get_json(&plain.url, "/api/fonts/index.json").await;
    assert_eq!(status, 404);
}
