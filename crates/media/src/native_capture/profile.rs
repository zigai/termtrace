use super::Result;
use std::path::{Path, PathBuf};
use termtrace_evidence::artifact::{create_private_directory, write_secure};

pub(super) fn materialize_profile(root: &Path, config: &[u8]) -> Result<PathBuf> {
    create_private_directory(root)?;
    let ghostty = root.join("ghostty");
    create_private_directory(&ghostty)?;
    write_secure(&ghostty.join("config.ghostty"), config)?;
    return Ok(root.to_path_buf());
}
