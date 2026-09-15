//! Offline access to the instructions shipped with this executable.

use std::path::{Component, Path};

use include_dir::{include_dir, Dir};

static BUNDLE: Dir<'_> = include_dir!("$CARGO_MANIFEST_DIR/../../skills");

pub(super) fn read(name: &str, file: &Path) -> Result<&'static str, String> {
    if name.is_empty()
        || name.contains(['/', '\\'])
        || name == "."
        || name == ".."
        || file.as_os_str().is_empty()
        || file
            .components()
            .any(|part| !matches!(part, Component::Normal(_)))
    {
        return Err("use a skill name and a relative file path without '..'".into());
    }
    BUNDLE
        .get_file(Path::new(name).join(file))
        .and_then(|file| file.contents_utf8())
        .ok_or_else(|| format!("no bundled skill file {name}/{}", file.display()))
}
