//! Where an installed generation keeps its files.

use std::path::{Path, PathBuf};

/// The directory a generation holds its files in: the generation itself when
/// it carries `bin/` at its root, otherwise the one directory inside it that
/// does (the platform tree a multi-platform generation installs into). The
/// layout is read from the generation rather than from a list of platform
/// names, so a generation built for any platform is found; one with no such
/// directory, or with several, is refused naming what it holds.
pub(crate) fn generation_root(generation: &Path) -> Result<PathBuf, String> {
    if generation.join("bin").is_dir() {
        return Ok(generation.to_path_buf());
    }
    let unlisted =
        |error: std::io::Error| format!("{} cannot be listed: {error}", generation.display());
    let mut trees = Vec::new();
    for entry in std::fs::read_dir(generation).map_err(unlisted)? {
        let path = entry.map_err(unlisted)?.path();
        if path.join("bin").is_dir() {
            trees.push(path);
        }
    }
    match trees.as_slice() {
        [] => Err(format!(
            "{} holds no bin directory, at its root or in a platform directory",
            generation.display()
        )),
        [tree] => Ok(tree.clone()),
        several => Err(format!(
            "{} holds several platform directories with a bin directory, so which one runs is not stated: {}",
            generation.display(),
            several
                .iter()
                .map(|tree| tree.display().to_string())
                .collect::<Vec<_>>()
                .join(", ")
        )),
    }
}
