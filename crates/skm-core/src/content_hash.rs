use std::{fs, io, path::Path};

use icu_collator::Collator;
use rayon::prelude::*;
use sha2::{Digest, Sha256};

/// Returns the lowercase SHA-256 hex digest used by `skills` computedHash.
///
/// Paths use UTF-8 and `/` separators, ordered by ICU's default root collation
/// (the standard English ordering). Upstream uses Node's environment-dependent
/// default locale; locales with different tailoring can produce different hashes.
/// Directory entries named `.git` or `node_modules`, symlinks, and special files
/// are ignored. File reads run in parallel; digest updates remain ordered.
pub fn content_hash(root: &Path) -> io::Result<String> {
    let mut files = Vec::new();
    collect_files(root, root, &mut files)?;
    let collator =
        Collator::try_new(Default::default(), Default::default()).map_err(io::Error::other)?;
    files.sort_by(|a, b| collator.compare(&a.0, &b.0));

    let contents: io::Result<Vec<_>> = files.par_iter().map(|(_, path)| fs::read(path)).collect();
    let mut hash = Sha256::new();
    for ((relative, _), bytes) in files.iter().zip(contents?) {
        hash.update(relative.as_bytes());
        hash.update(bytes);
    }
    Ok(format!("{:x}", hash.finalize()))
}

/// Directories that never count as skill content, matching npx skills.
pub fn is_ignored_dir(name: &std::ffi::OsStr) -> bool {
    name == ".git" || name == "node_modules"
}

fn collect_files(
    root: &Path,
    directory: &Path,
    files: &mut Vec<(String, std::path::PathBuf)>,
) -> io::Result<()> {
    for entry in fs::read_dir(directory)? {
        let entry = entry?;
        let kind = entry.file_type()?;
        let path = entry.path();
        if kind.is_dir() {
            if !is_ignored_dir(&entry.file_name()) {
                collect_files(root, &path, files)?;
            }
        } else if kind.is_file() {
            let relative = path.strip_prefix(root).map_err(io::Error::other)?;
            files.push((relative.to_string_lossy().replace('\\', "/"), path));
        }
    }
    Ok(())
}
