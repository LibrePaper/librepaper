//! Offline access to the instructions shipped with this executable.

use std::path::{Component, Path, PathBuf};

use clap::Subcommand;
use include_dir::{include_dir, Dir};

static BUNDLE: Dir<'_> = include_dir!("$CARGO_MANIFEST_DIR/../../skills");

#[derive(Subcommand)]
pub(crate) enum SkillsCommand {
    /// List bundled skills and the application version
    List,
    /// Print a skill or one of its reference files as Markdown
    Show {
        name: String,
        /// Path relative to the skill directory, e.g. references/editing.md
        #[arg(long, default_value = "SKILL.md")]
        file: PathBuf,
    },
    /// Export all skills and references into a new directory
    Export {
        /// Must not already exist; use a fresh directory after upgrading
        #[arg(long)]
        directory: PathBuf,
    },
}

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
        .ok_or_else(|| {
            format!(
                "no bundled skill file {name}/{}; run librepaper skills list",
                file.display()
            )
        })
}

pub(super) fn run(command: SkillsCommand) -> Result<(), String> {
    match command {
        SkillsCommand::List => {
            println!("LibrePaper {}", crate::VERSION);
            for directory in BUNDLE.dirs() {
                println!("{}", directory.path().display());
            }
        }
        SkillsCommand::Show { name, file } => print!("{}", read(&name, &file)?),
        SkillsCommand::Export { directory } => {
            // Refuse existing directories, including symlinks, so exporting an
            // update cannot silently replace an agent's customized instructions.
            std::fs::create_dir(&directory)
                .map_err(|error| format!("could not create {}: {error}", directory.display()))?;
            BUNDLE.extract(&directory).map_err(|error| {
                format!(
                    "could not export skills to {}: {error}",
                    directory.display()
                )
            })?;
            println!(
                "Exported LibrePaper {} skills to {}",
                crate::VERSION,
                directory.display()
            );
        }
    }
    Ok(())
}
