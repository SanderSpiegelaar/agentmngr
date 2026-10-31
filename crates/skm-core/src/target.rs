use std::{
    collections::VecDeque,
    ffi::OsString,
    fs, io,
    path::{Component, Path, PathBuf},
};

use sha2::{Digest, Sha256};

use crate::{Error, content_hash::content_hash, paths::Paths};

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum ItemKind {
    Skill,
    Mcp,
    Hook,
    Subagent,
    Plugin,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ItemRef {
    pub kind: ItemKind,
    pub name: String,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum TargetClass {
    Absent,
    OurLink(ItemRef),
    ForeignLink(PathBuf),
    RealDir,
    RealFile,
}

/// Classify the target itself, not the file or directory behind a symlink.
/// Broken links to item roots remain ours; links to library metadata or item
/// descendants are foreign because they do not identify a complete item.
/// Foreign links retain the target as written, including relative paths.
pub fn classify(path: &Path, paths: &Paths) -> Result<TargetClass, Error> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(TargetClass::Absent),
        Err(error) => return Err(error.into()),
    };
    if metadata.is_symlink() {
        let target = fs::read_link(path)?;
        return classify_link(path, &target, paths);
    }
    if metadata.is_dir() {
        Ok(TargetClass::RealDir)
    } else if metadata.is_file() {
        Ok(TargetClass::RealFile)
    } else {
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "target is not a file or directory",
        )
        .into())
    }
}

pub(crate) fn classify_link(
    path: &Path,
    target: &Path,
    paths: &Paths,
) -> Result<TargetClass, Error> {
    let absolute = if target.is_absolute() {
        target.to_owned()
    } else {
        path.parent().unwrap_or(Path::new(".")).join(target)
    };
    let resolved = resolve_link_path(&absolute)?;
    let library = resolve_link_path(&paths.library)?;
    if let Ok(relative) = resolved.strip_prefix(library)
        && let Some(item) = item_ref(relative)
    {
        return Ok(TargetClass::OurLink(item));
    }
    Ok(TargetClass::ForeignLink(target.to_owned()))
}

fn item_ref(relative: &Path) -> Option<ItemRef> {
    let mut components = relative.components();
    let directory = components.next()?.as_os_str().to_str()?;
    let filename = components.next()?.as_os_str().to_str()?;
    if components.next().is_some() {
        return None;
    }
    let (kind, name) = match directory {
        "skills" => (ItemKind::Skill, filename),
        "mcp" => (ItemKind::Mcp, filename.strip_suffix(".json")?),
        "hooks" => (ItemKind::Hook, filename.strip_suffix(".json")?),
        "subagents" => (ItemKind::Subagent, filename.strip_suffix(".md")?),
        "plugins" if !filename.starts_with('.') => (ItemKind::Plugin, filename),
        _ => return None,
    };
    if name.is_empty() {
        return None;
    }
    Some(ItemRef {
        kind,
        name: name.to_owned(),
    })
}

// Resolve existing symlinks without requiring the final item to exist. Process
// `..` after symlinks so a link cannot disguise an escape from the library.
fn resolve_link_path(path: &Path) -> io::Result<PathBuf> {
    let absolute = std::path::absolute(path)?;
    let mut pending: VecDeque<OsString> = absolute
        .components()
        .map(|part| part.as_os_str().to_owned())
        .collect();
    let mut resolved = PathBuf::new();
    let mut links = 0;
    while let Some(part) = pending.pop_front() {
        match Path::new(&part).components().next().unwrap() {
            Component::RootDir | Component::Prefix(_) => resolved.push(part),
            Component::CurDir => {}
            Component::ParentDir => {
                resolved.pop();
            }
            Component::Normal(_) => {
                resolved.push(part);
                match fs::symlink_metadata(&resolved) {
                    Ok(metadata) if metadata.is_symlink() => {
                        links += 1;
                        if links > 40 {
                            return Err(io::Error::other("too many symbolic links"));
                        }
                        let target = fs::read_link(&resolved)?;
                        resolved.pop();
                        if target.is_absolute() {
                            resolved.clear();
                        }
                        for part in target.components().rev() {
                            pending.push_front(part.as_os_str().to_owned());
                        }
                    }
                    Ok(_) => {}
                    Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                    Err(error) => return Err(error),
                }
            }
        }
    }
    Ok(resolved)
}

/// Compare a real copy with a library item, without treating links as copies.
/// Directory hashes follow the same exclusions as `content_hash`.
pub fn copy_matches_library_item(copy: &Path, library_item: &Path) -> Result<bool, Error> {
    let copy_type = fs::symlink_metadata(copy)?.file_type();
    let item_type = fs::symlink_metadata(library_item)?.file_type();
    if copy_type.is_dir() && item_type.is_dir() {
        Ok(content_hash(copy)? == content_hash(library_item)?)
    } else if copy_type.is_file() && item_type.is_file() {
        Ok(Sha256::digest(fs::read(copy)?) == Sha256::digest(fs::read(library_item)?))
    } else {
        Ok(false)
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::os::unix::fs::symlink;
    use tempfile::TempDir;

    fn fixture() -> (TempDir, Paths) {
        let temp = tempfile::tempdir().unwrap();
        let paths = Paths::resolve(Some(&temp.path().join("home")), None, None).unwrap();
        paths.ensure_layout().unwrap();
        (temp, paths)
    }

    fn skill() -> TargetClass {
        TargetClass::OurLink(ItemRef {
            kind: ItemKind::Skill,
            name: "review".into(),
        })
    }

    #[test]
    fn absent() {
        let (temp, paths) = fixture();
        assert_eq!(
            classify(&temp.path().join("absent"), &paths).unwrap(),
            TargetClass::Absent
        );
    }

    #[test]
    fn our_absolute_link() {
        let (temp, paths) = fixture();
        let item = paths.library.join("skills/review");
        fs::create_dir_all(&item).unwrap();
        let link = temp.path().join("link");
        symlink(item, &link).unwrap();
        assert_eq!(classify(&link, &paths).unwrap(), skill());
    }

    #[test]
    fn our_relative_link() {
        let (temp, paths) = fixture();
        fs::create_dir_all(paths.library.join("skills/review")).unwrap();
        fs::create_dir(temp.path().join("agent")).unwrap();
        let link = temp.path().join("agent/link");
        symlink("../home/library/skills/./review", &link).unwrap();
        assert_eq!(classify(&link, &paths).unwrap(), skill());
    }

    #[test]
    fn broken_link_into_library_is_still_our_link() {
        let (temp, paths) = fixture();
        let link = temp.path().join("link");
        symlink(paths.library.join("skills/review"), &link).unwrap();
        assert!(!link.exists());
        assert_eq!(classify(&link, &paths).unwrap(), skill());
    }

    #[test]
    fn another_tools_link_and_broken_foreign_link() {
        let (temp, paths) = fixture();
        let target = temp.path().join("other-tool/review");
        fs::create_dir_all(&target).unwrap();
        let link = temp.path().join("link");
        symlink(&target, &link).unwrap();
        assert_eq!(
            classify(&link, &paths).unwrap(),
            TargetClass::ForeignLink(target.clone())
        );
        fs::remove_dir(&target).unwrap();
        assert_eq!(
            classify(&link, &paths).unwrap(),
            TargetClass::ForeignLink(target)
        );
    }

    #[test]
    fn real_directory_and_file() {
        let (temp, paths) = fixture();
        let dir = temp.path().join("directory");
        let file = temp.path().join("file");
        fs::create_dir(&dir).unwrap();
        fs::write(&file, "content").unwrap();
        assert_eq!(classify(&dir, &paths).unwrap(), TargetClass::RealDir);
        assert_eq!(classify(&file, &paths).unwrap(), TargetClass::RealFile);
    }

    #[test]
    fn all_library_item_kinds_and_unmanaged_paths() {
        let (temp, paths) = fixture();
        for (relative, kind, name) in [
            ("skills/review", ItemKind::Skill, "review"),
            ("mcp/github.json", ItemKind::Mcp, "github"),
            ("hooks/format.json", ItemKind::Hook, "format"),
            ("subagents/reviewer.md", ItemKind::Subagent, "reviewer"),
            ("plugins/tools", ItemKind::Plugin, "tools"),
        ] {
            let link = temp.path().join("link");
            symlink(paths.library.join(relative), &link).unwrap();
            assert_eq!(
                classify(&link, &paths).unwrap(),
                TargetClass::OurLink(ItemRef {
                    kind,
                    name: name.into()
                })
            );
            fs::remove_file(link).unwrap();
        }
        for relative in [
            "library.toml",
            "skills",
            "skills/review/SKILL.md",
            "mcp/bad",
            "plugins/.claude-plugin",
        ] {
            let link = temp.path().join("link");
            let target = paths.library.join(relative);
            symlink(&target, &link).unwrap();
            assert_eq!(
                classify(&link, &paths).unwrap(),
                TargetClass::ForeignLink(target)
            );
            fs::remove_file(link).unwrap();
        }
    }

    #[test]
    fn library_prefix_and_symlink_escape_are_foreign() {
        let (temp, paths) = fixture();
        fs::create_dir_all(paths.library.join("skills")).unwrap();
        let outside = temp.path().join("outside");
        fs::create_dir(&outside).unwrap();
        symlink(&outside, paths.library.join("skills/review")).unwrap();
        for target in [
            paths.library.join("skills/review"),
            paths.home.join("library-other/skills/review"),
        ] {
            let link = temp.path().join("link");
            symlink(&target, &link).unwrap();
            assert_eq!(
                classify(&link, &paths).unwrap(),
                TargetClass::ForeignLink(target)
            );
            fs::remove_file(link).unwrap();
        }
    }

    #[test]
    fn symlink_is_resolved_before_parent_traversal() {
        let (temp, paths) = fixture();
        fs::create_dir_all(paths.library.join("skills")).unwrap();
        let outside = temp.path().join("outside/nested");
        fs::create_dir_all(&outside).unwrap();
        symlink(&outside, paths.library.join("skills/alias")).unwrap();
        let link = temp.path().join("link");
        let target = paths.library.join("skills/alias/../review");
        symlink(&target, &link).unwrap();
        assert_eq!(
            classify(&link, &paths).unwrap(),
            TargetClass::ForeignLink(target)
        );
    }

    #[test]
    fn symlinked_library_and_indirect_link_resolve_to_our_item() {
        let (temp, mut paths) = fixture();
        fs::create_dir_all(paths.library.join("skills/review")).unwrap();
        let alias = temp.path().join("library-alias");
        symlink(&paths.library, &alias).unwrap();
        let intermediate = temp.path().join("intermediate");
        symlink(paths.library.join("skills/review"), &intermediate).unwrap();
        paths.library = alias;
        let link = temp.path().join("link");
        symlink("intermediate", &link).unwrap();
        assert_eq!(classify(&link, &paths).unwrap(), skill());
    }

    #[test]
    fn matching_directory_copy_and_modified_copy() {
        let (temp, paths) = fixture();
        let item = paths.library.join("skills/review");
        let copy = temp.path().join("copy");
        for dir in [&item, &copy] {
            fs::create_dir_all(dir.join("scripts")).unwrap();
            fs::write(dir.join("SKILL.md"), "review").unwrap();
            fs::write(dir.join("scripts/check.sh"), "check").unwrap();
        }
        assert!(copy_matches_library_item(&copy, &item).unwrap());
        fs::write(copy.join("SKILL.md"), "modified").unwrap();
        assert!(!copy_matches_library_item(&copy, &item).unwrap());
    }

    #[test]
    fn matching_file_copy_type_mismatch_and_symlink_rejection() {
        let (temp, paths) = fixture();
        let item = paths.library.join("server.json");
        let copy = temp.path().join("copy.json");
        fs::write(&item, "{}").unwrap();
        fs::write(&copy, "{}").unwrap();
        assert!(copy_matches_library_item(&copy, &item).unwrap());
        fs::write(&copy, "different").unwrap();
        assert!(!copy_matches_library_item(&copy, &item).unwrap());
        assert!(!copy_matches_library_item(temp.path(), &item).unwrap());
        let link = temp.path().join("link");
        symlink(&item, &link).unwrap();
        assert!(!copy_matches_library_item(&link, &item).unwrap());
    }

    #[test]
    fn symlink_loop_and_missing_copy_return_core_errors() {
        let (temp, paths) = fixture();
        let link = temp.path().join("loop");
        symlink("loop", &link).unwrap();
        assert_eq!(classify(&link, &paths).unwrap_err().code(), "E_IO");
        assert_eq!(
            copy_matches_library_item(&temp.path().join("missing"), temp.path())
                .unwrap_err()
                .code(),
            "E_IO"
        );
    }
}
