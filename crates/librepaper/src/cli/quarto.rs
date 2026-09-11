//! Quarto source inspection. These commands never execute or retain results.

use std::io::Read;
use std::path::{Path, PathBuf};

use clap::Subcommand;
use serde_json::json;

#[derive(Subcommand)]
pub enum QuartoCommand {
    /// Inspect source cells and fingerprints without running code
    Inspect {
        file: PathBuf,
        /// Project-relative identity of this source file
        #[arg(long)]
        main: Option<String>,
    },
}

fn read_bounded(path: &Path) -> Result<Vec<u8>, String> {
    let metadata =
        std::fs::metadata(path).map_err(|error| format!("{}: {error}", path.display()))?;
    if !metadata.is_file() || metadata.len() > crate::quarto::MAX_BUNDLE_BYTES as u64 {
        return Err(format!("{} is not a bounded regular file", path.display()));
    }
    let mut bytes = Vec::new();
    std::fs::File::open(path)
        .map_err(|error| format!("{}: {error}", path.display()))?
        .take(crate::quarto::MAX_BUNDLE_BYTES as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| format!("{}: {error}", path.display()))?;
    if bytes.len() > crate::quarto::MAX_BUNDLE_BYTES {
        return Err("input grew beyond its size limit".into());
    }
    Ok(bytes)
}

pub async fn run(
    command: QuartoCommand,
    _server: Option<String>,
    _token: Option<String>,
) -> Result<(), String> {
    match command {
        QuartoCommand::Inspect { file, main } => {
            let bytes = read_bounded(&file)?;
            let source = std::str::from_utf8(&bytes).map_err(|_| "Quarto source must be UTF-8")?;
            let main = main.unwrap_or_else(|| {
                file.file_name()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .to_string()
            });
            let document = crate::quarto::parse_qmd(source, &main);
            let cells: Vec<_> = document.cells.iter().map(|cell| json!({
                "id":cell.id,"label":cell.label,"language":cell.language,"options":cell.options,
                "source":cell.source,"source_sha256":cell.source_sha256,
                "start_line":cell.start_line,"end_line":cell.end_line
            })).collect();
            let context = crate::quarto::computation_fingerprint(&document, &main, &[], None);
            println!(
                "{}",
                serde_json::to_string_pretty(&json!({"schema":1,"main":main,"cells":cells,
                "computation_sha256":context,"diagnostics":document.diagnostics}))
                .map_err(|error| error.to_string())?
            );
        }
    }
    Ok(())
}
