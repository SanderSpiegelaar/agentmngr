use std::{
    fs::{self, File},
    io::{self, Write},
    path::Path,
};

use sha2::{Digest, Sha256};

use super::{Action, TargetState};
use crate::{
    Error,
    paths::Paths,
    target::{TargetClass, classify},
};

pub fn snapshot(path: &Path, paths: &Paths) -> Result<TargetState, Error> {
    let class = classify(path, paths)?;
    let hash = if class == TargetClass::Absent {
        None
    } else {
        Some(fingerprint(path)?)
    };
    let mode = if matches!(class, TargetClass::RealFile | TargetClass::RealDir) {
        permission_bits(&fs::symlink_metadata(path)?)
    } else {
        None
    };
    Ok(TargetState { class, mode, hash })
}

pub(super) fn file_hash(bytes: &[u8], mode: Option<u32>) -> String {
    let mut hash = Sha256::new();
    hash.update(b"file\0");
    if let Some(mode) = mode {
        hash.update(mode.to_le_bytes());
    }
    hash.update(bytes);
    format!("{:x}", hash.finalize())
}

fn permission_bits(metadata: &fs::Metadata) -> Option<u32> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        Some(metadata.permissions().mode() & 0o7777)
    }
    #[cfg(not(unix))]
    {
        let _ = metadata;
        None
    }
}

pub(crate) fn link_hash(to: &Path) -> String {
    let mut hash = Sha256::new();
    hash.update(b"link\0");
    hash.update(to.as_os_str().as_encoded_bytes());
    format!("{:x}", hash.finalize())
}

pub(super) fn fingerprint(path: &Path) -> io::Result<String> {
    let meta = fs::symlink_metadata(path)?;
    if meta.is_symlink() {
        return Ok(link_hash(&fs::read_link(path)?));
    }
    if meta.is_file() {
        return Ok(file_hash(&fs::read(path)?, permission_bits(&meta)));
    }
    if !meta.is_dir() {
        return Err(io::Error::other("unsupported filesystem entry"));
    }
    let mut hash = Sha256::new();
    hash.update(b"directory\0");
    if let Some(mode) = permission_bits(&meta) {
        hash.update(mode.to_le_bytes());
    }
    let mut children = fs::read_dir(path)?.collect::<Result<Vec<_>, _>>()?;
    children.sort_by_key(|child| child.file_name());
    for child in children {
        let name = child.file_name();
        let name = name.as_encoded_bytes();
        hash.update((name.len() as u64).to_le_bytes());
        hash.update(name);
        hash.update(fingerprint(&child.path())?);
    }
    Ok(format!("{:x}", hash.finalize()))
}

pub(super) fn parents_safe(target: &Path) -> io::Result<bool> {
    if !target.is_absolute()
        || target
            .components()
            .any(|part| matches!(part, std::path::Component::ParentDir))
    {
        return Ok(false);
    }
    for ancestor in target.ancestors().skip(1) {
        match fs::symlink_metadata(ancestor) {
            Ok(meta) if !meta.is_dir() || meta.is_symlink() => return Ok(false),
            Ok(_) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
    }
    Ok(target.file_name().is_some())
}

pub(super) fn copy(source: &Path, target: &Path) -> io::Result<()> {
    let meta = fs::symlink_metadata(source)?;
    if meta.is_symlink() {
        symlink(&fs::read_link(source)?, target)?;
    } else if meta.is_dir() {
        fs::create_dir(target)?;
        for entry in fs::read_dir(source)? {
            let entry = entry?;
            copy(&entry.path(), &target.join(entry.file_name()))?;
        }
        fs::set_permissions(target, meta.permissions())?;
        File::open(target)?.sync_all()?;
    } else if meta.is_file() {
        fs::copy(source, target)?;
        File::open(target)?.sync_all()?;
    } else {
        return Err(io::Error::other("unsupported filesystem entry"));
    }
    Ok(())
}

#[cfg(unix)]
fn symlink(to: &Path, target: &Path) -> io::Result<()> {
    std::os::unix::fs::symlink(to, target)
}

#[cfg(not(unix))]
fn symlink(_to: &Path, _target: &Path) -> io::Result<()> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "symlink deployment requires Unix",
    ))
}

/// Backups are durable before replacing anything. Directories may briefly be absent,
/// but staging plus rename ensures a reader never sees a partially copied tree.
pub(super) fn execute(
    action: &Action,
    backup: &Path,
    existed: bool,
    paths: &Paths,
    expected: &TargetState,
) -> Result<(), Error> {
    let target = action.target();
    if matches!(action, Action::Claim { .. }) {
        if !parents_safe(target)? || snapshot(target, paths)? != *expected {
            return Err(Error::ConfigChanged(
                "project copy changed before claim".into(),
            ));
        }
        return Ok(());
    }
    let parent = target
        .parent()
        .ok_or_else(|| io::Error::other("target has no parent"))?;
    fs::create_dir_all(parent)?;
    if let Action::Trash { .. } = action {
        if !parents_safe(target)? || snapshot(target, paths)? != *expected {
            return Err(Error::ConfigChanged(
                "target changed before trash move".into(),
            ));
        }
        fs::rename(target, backup)?;
        File::open(parent)?.sync_all()?;
        File::open(backup.parent().unwrap())?.sync_all()?;
        return Ok(());
    }
    let staging = tempfile::Builder::new()
        .prefix(".skm-")
        .suffix(".skm-tmp")
        .tempdir_in(parent)?;
    let file_staging = if matches!(action, Action::WriteFile { .. }) {
        Some(
            tempfile::Builder::new()
                .prefix(".skm-")
                .suffix(".skm-tmp")
                .tempfile_in(parent)?,
        )
    } else {
        None
    };
    let staged = file_staging.as_ref().map_or_else(
        || staging.path().join("target"),
        |file| file.path().to_path_buf(),
    );
    match action {
        Action::Link { to, .. } => symlink(to, &staged)?,
        Action::WriteFile { bytes, .. } => {
            let mut file = File::create(&staged)?;
            file.write_all(bytes)?;
            if existed && fs::symlink_metadata(target)?.is_file() {
                file.set_permissions(fs::metadata(target)?.permissions())?;
            }
            file.sync_all()?;
        }
        Action::CopyTree { from, hash, .. } | Action::Restore { from, hash, .. } => {
            if matches!(action, Action::CopyTree { .. }) && !fs::symlink_metadata(from)?.is_dir() {
                return Err(Error::ForeignTarget(
                    "tree source is not a real directory".into(),
                ));
            }
            if fingerprint(from)? != *hash {
                return Err(Error::ConfigChanged(
                    "copy source changed since planning".into(),
                ));
            }
            copy(from, &staged)?;
            if fingerprint(&staged)? != *hash || fingerprint(from)? != *hash {
                return Err(Error::ConfigChanged(
                    "copy source changed during staging".into(),
                ));
            }
            if let Action::CopyTree {
                computed_hash: Some(hash),
                ..
            } = action
                && crate::content_hash::content_hash(&staged)? != *hash
            {
                return Err(Error::ConfigChanged(
                    "adopted content hash differs from original".into(),
                ));
            }
        }
        Action::Trash { .. } | Action::Claim { .. } => unreachable!(),
    }
    if !parents_safe(target)? || snapshot(target, paths)? != *expected {
        return Err(Error::ConfigChanged("target changed during staging".into()));
    }
    let move_old = existed
        && (fs::symlink_metadata(target)?.is_dir() || fs::symlink_metadata(&staged)?.is_dir());
    if existed {
        if move_old {
            // An existing nonempty directory cannot be atomically overwritten by rename.
            fs::rename(target, backup)?;
        } else {
            copy(target, backup)?;
            if snapshot(target, paths)? != *expected {
                return Err(Error::ConfigChanged("target changed during backup".into()));
            }
        }
        File::open(backup.parent().unwrap())?.sync_all()?;
    }
    if existed && fingerprint(backup)? != expected.hash.as_deref().unwrap_or_default() {
        if move_old {
            fs::rename(backup, target)?;
        }
        return Err(Error::ConfigChanged(
            "target changed while being backed up".into(),
        ));
    }
    if let Err(error) = fs::rename(staged, target) {
        if move_old {
            fs::rename(backup, target)?;
        }
        return Err(error.into());
    }
    File::open(parent)?.sync_all()?;
    Ok(())
}
