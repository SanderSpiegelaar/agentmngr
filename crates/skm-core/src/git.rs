use std::{
    ffi::{OsStr, OsString},
    io::{self, Read},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::mpsc,
    thread,
    time::{Duration, Instant},
};

use crate::Error;

#[derive(Debug, Clone)]
pub struct GitRunner {
    binary: PathBuf,
    timeout: Duration,
}

#[derive(Debug, Clone)]
pub struct BareRepository {
    runner: GitRunner,
    path: PathBuf,
}

/// Project handles expose no arbitrary commands or index/history writes.
///
/// ```compile_fail
/// use skm_core::git::ProjectRepository;
/// fn stage(project: &ProjectRepository) { project.add("."); }
/// ```
/// ```compile_fail
/// use skm_core::git::ProjectRepository;
/// fn commit(project: &ProjectRepository) { project.commit("message"); }
/// ```
#[derive(Debug, Clone)]
pub struct ProjectRepository {
    runner: GitRunner,
    path: PathBuf,
}

impl GitRunner {
    pub fn new(timeout: Duration) -> Result<Self, Error> {
        Self::with_binary("git", timeout)
    }

    pub fn with_binary(binary: impl Into<PathBuf>, timeout: Duration) -> Result<Self, Error> {
        let runner = Self {
            binary: binary.into(),
            timeout,
        };
        let output = runner.run(None, ["--version"])?;
        let version = String::from_utf8_lossy(&output);
        let mut parts = version
            .trim()
            .strip_prefix("git version ")
            .unwrap_or("")
            .split('.');
        let major = parts.next().and_then(|part| part.parse::<u32>().ok());
        let minor = parts.next().and_then(|part| part.parse::<u32>().ok());
        if !matches!((major, minor), (Some(major), Some(minor)) if (major, minor) >= (2, 30)) {
            return Err(Error::GitMissing(format!(
                "Git >=2.30 is required; found {}",
                version.trim()
            )));
        }
        Ok(runner)
    }

    /// Clones the remote HEAD. Use `BareRepository::fetch` for other branches, tags or SHAs.
    pub fn clone_bare(&self, url: &str, destination: &Path) -> Result<BareRepository, Error> {
        let args: [&OsStr; 8] = [
            "clone".as_ref(),
            "--bare".as_ref(),
            "--filter=blob:none".as_ref(),
            "--depth".as_ref(),
            "1".as_ref(),
            "--".as_ref(),
            url.as_ref(),
            destination.as_os_str(),
        ];
        self.run(None, args)?;
        self.bare(destination)
    }

    pub fn bare(&self, path: &Path) -> Result<BareRepository, Error> {
        let path = std::fs::canonicalize(path)?;
        if self.run(Some(&path), ["rev-parse", "--is-bare-repository"])? != b"true\n" {
            return Err(io::Error::other("expected a bare Git repository").into());
        }
        Ok(BareRepository {
            runner: self.clone(),
            path,
        })
    }

    pub fn project(&self, path: &Path) -> Result<ProjectRepository, Error> {
        let path = std::fs::canonicalize(path)?;
        let root = self.run(Some(&path), ["rev-parse", "--show-toplevel"])?;
        Ok(ProjectRepository {
            runner: self.clone(),
            path: output_path(trim_newline(&root))?,
        })
    }

    fn run<I, S>(&self, directory: Option<&Path>, args: I) -> Result<Vec<u8>, Error>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        let start = Instant::now();
        let mut command = Command::new(&self.binary);
        command
            .args(args)
            .env("GIT_TERMINAL_PROMPT", "0")
            .env("LC_ALL", "C")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        if let Some(directory) = directory {
            command.current_dir(directory);
        }
        let mut child = command.spawn().map_err(|error| {
            if error.kind() == io::ErrorKind::NotFound {
                Error::GitMissing(format!(
                    "Git executable {} is unavailable",
                    self.binary.display()
                ))
            } else {
                error.into()
            }
        })?;
        let (sender, receiver) = mpsc::channel();
        for (stderr, mut pipe) in [
            (
                false,
                Box::new(child.stdout.take().unwrap()) as Box<dyn Read + Send>,
            ),
            (
                true,
                Box::new(child.stderr.take().unwrap()) as Box<dyn Read + Send>,
            ),
        ] {
            let sender = sender.clone();
            thread::spawn(move || {
                let mut bytes = Vec::new();
                let result = pipe.read_to_end(&mut bytes).map(|_| bytes);
                let _ = sender.send((stderr, result));
            });
        }
        drop(sender);
        let status = loop {
            match child.try_wait() {
                Ok(Some(status)) => break status,
                Ok(None) if start.elapsed() < self.timeout => thread::sleep(
                    Duration::from_millis(5).min(self.timeout.saturating_sub(start.elapsed())),
                ),
                result => {
                    let _ = child.kill();
                    let _ = child.wait();
                    return match result {
                        Err(error) => Err(error.into()),
                        _ => Err(timeout_error(self.timeout)),
                    };
                }
            }
        };
        let mut stdout = Vec::new();
        let mut stderr = Vec::new();
        for _ in 0..2 {
            let (is_stderr, bytes) = receiver
                .recv_timeout(self.timeout.saturating_sub(start.elapsed()))
                .map_err(|_| timeout_error(self.timeout))?;
            if is_stderr {
                stderr = bytes?;
            } else {
                stdout = bytes?;
            }
        }
        if status.success() {
            return Ok(stdout);
        }
        let message = String::from_utf8_lossy(&stderr).trim().to_owned();
        let lower = message.to_ascii_lowercase();
        if lower.contains("xcode-select:") || lower.contains("invalid active developer path") {
            Err(Error::GitMissing(message))
        } else if [
            "authentication failed",
            "permission denied",
            "could not read username",
            "terminal prompts disabled",
            "authorization failed",
            "http 401",
            "http 403",
            "returned error: 401",
            "returned error: 403",
        ]
        .iter()
        .any(|needle| lower.contains(needle))
        {
            Err(Error::GitAuth(message))
        } else if [
            "could not resolve",
            "unable to access",
            "connection refused",
            "connection timed out",
            "network is unreachable",
            "could not read from remote repository",
        ]
        .iter()
        .any(|needle| lower.contains(needle))
        {
            Err(Error::Network(message))
        } else {
            Err(io::Error::other(format!("Git exited with {status}: {message}")).into())
        }
    }
}

impl BareRepository {
    pub fn fetch(&self, revision: Option<&str>) -> Result<(), Error> {
        let mut args = vec![
            "fetch",
            "--depth",
            "1",
            "--filter=blob:none",
            "--",
            "origin",
        ];
        if let Some(revision) = revision {
            args.push(revision);
        }
        self.runner.run(Some(&self.path), args).map(|_| ())
    }

    pub fn ls_tree(&self, revision: &str) -> Result<Vec<PathBuf>, Error> {
        validate_revision(revision)?;
        paths_from_output(&self.runner.run(
            Some(&self.path),
            ["ls-tree", "-r", "--name-only", "-z", revision, "--"],
        )?)
    }

    pub fn archive(&self, revision: &str, path: &Path) -> Result<Vec<u8>, Error> {
        validate_revision(revision)?;
        self.runner.run(
            Some(&self.path),
            [
                OsStr::new("archive"),
                OsStr::new("--format=tar"),
                OsStr::new(revision),
                OsStr::new("--"),
                path.as_os_str(),
            ],
        )
    }

    pub fn rev_parse(&self, revision: &str) -> Result<String, Error> {
        let output = self.runner.run(
            Some(&self.path),
            ["rev-parse", "--verify", "--end-of-options", revision],
        )?;
        String::from_utf8(trim_newline(&output).to_vec())
            .map_err(|error| io::Error::other(error).into())
    }
}

impl ProjectRepository {
    pub fn ls_files(&self) -> Result<Vec<PathBuf>, Error> {
        paths_from_output(
            &self
                .runner
                .run(Some(&self.path), ["ls-files", "--cached", "-z"])?,
        )
    }

    pub fn exclude_path(&self) -> Result<PathBuf, Error> {
        let output = self.runner.run(
            Some(&self.path),
            ["rev-parse", "--git-path", "info/exclude"],
        )?;
        Ok(self.path.join(output_path(trim_newline(&output))?))
    }
}

fn validate_revision(revision: &str) -> Result<(), Error> {
    if revision.is_empty() || revision.starts_with('-') {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, "invalid Git revision").into());
    }
    Ok(())
}

fn timeout_error(timeout: Duration) -> Error {
    Error::Network(format!("Git timed out after {timeout:?}"))
}

fn trim_newline(bytes: &[u8]) -> &[u8] {
    bytes.strip_suffix(b"\n").unwrap_or(bytes)
}

fn paths_from_output(output: &[u8]) -> Result<Vec<PathBuf>, Error> {
    output
        .split(|byte| *byte == 0)
        .filter(|path| !path.is_empty())
        .map(output_path)
        .collect()
}

fn output_path(bytes: &[u8]) -> Result<PathBuf, Error> {
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStringExt;
        Ok(OsString::from_vec(bytes.to_vec()).into())
    }
    #[cfg(not(unix))]
    {
        Ok(PathBuf::from(
            std::str::from_utf8(bytes).map_err(io::Error::other)?,
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn git(directory: &Path, args: &[&str]) -> Vec<u8> {
        let output = Command::new("git")
            .current_dir(directory)
            .env("HOME", directory)
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_GLOBAL", directory.join("missing-config"))
            .args([
                "-c",
                "user.name=Test",
                "-c",
                "user.email=test@example.invalid",
                "-c",
                "commit.gpgsign=false",
                "-c",
                "core.hooksPath=/dev/null",
            ])
            .args(args)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        output.stdout
    }

    fn source(root: &Path) -> PathBuf {
        let source = root.join("source");
        fs::create_dir(&source).unwrap();
        git(&source, &["init", "-b", "main"]);
        fs::create_dir(source.join("skills")).unwrap();
        fs::write(source.join("skills/SKILL.md"), "first skill\n").unwrap();
        fs::write(source.join("space\nand tab\t.txt"), "odd path").unwrap();
        git(&source, &["add", "."]);
        git(&source, &["commit", "-m", "initial"]);
        source
    }

    #[test]
    fn local_bare_clone_fetch_tree_revision_and_archive() {
        let root = tempfile::tempdir().unwrap();
        let source = source(root.path());
        let remote = root.path().join("remote.git");
        git(
            root.path(),
            &[
                "clone",
                "--bare",
                source.to_str().unwrap(),
                remote.to_str().unwrap(),
            ],
        );
        let runner = GitRunner::new(Duration::from_secs(10)).unwrap();
        let destination = root.path().join("cache.git");
        let repo = runner
            .clone_bare(&format!("file://{}", remote.display()), &destination)
            .unwrap();
        assert_eq!(
            git(&destination, &["rev-parse", "--is-shallow-repository"]),
            b"true\n"
        );
        assert_eq!(
            git(
                &destination,
                &["config", "remote.origin.partialclonefilter"]
            ),
            b"blob:none\n"
        );
        assert_eq!(
            repo.ls_tree("HEAD").unwrap(),
            vec![
                PathBuf::from("skills/SKILL.md"),
                PathBuf::from("space\nand tab\t.txt")
            ]
        );
        let before = repo.rev_parse("HEAD").unwrap();
        let archive = repo.archive("HEAD", Path::new("skills")).unwrap();
        let tar = root.path().join("skill.tar");
        fs::write(&tar, archive).unwrap();
        let unpacked = root.path().join("unpacked");
        fs::create_dir(&unpacked).unwrap();
        assert!(
            Command::new("tar")
                .args(["-xf"])
                .arg(&tar)
                .arg("-C")
                .arg(&unpacked)
                .status()
                .unwrap()
                .success()
        );
        assert_eq!(
            fs::read_to_string(unpacked.join("skills/SKILL.md")).unwrap(),
            "first skill\n"
        );
        assert!(!unpacked.join("space\nand tab\t.txt").exists());
        fs::write(source.join("skills/SKILL.md"), "second skill\n").unwrap();
        git(&source, &["commit", "-am", "update"]);
        git(&source, &["push", remote.to_str().unwrap(), "main"]);
        repo.fetch(Some("main")).unwrap();
        assert_ne!(repo.rev_parse("FETCH_HEAD").unwrap(), before);
        assert!(repo.archive("--output=bad", Path::new("skills")).is_err());
        assert!(repo.ls_tree("--help").is_err());
        assert!(runner.bare(&source).is_err());
    }

    #[test]
    fn project_lists_tracked_paths_without_writes_and_resolves_worktree_excludes() {
        let root = tempfile::tempdir().unwrap();
        let source = source(root.path());
        fs::write(source.join("untracked"), "leave alone").unwrap();
        let runner = GitRunner::new(Duration::from_secs(10)).unwrap();
        let index_before = fs::read(source.join(".git/index")).unwrap();
        let head_before = git(&source, &["rev-parse", "HEAD"]);
        let project = runner.project(&source.join("skills")).unwrap();
        assert_eq!(
            project.ls_files().unwrap(),
            vec![
                PathBuf::from("skills/SKILL.md"),
                PathBuf::from("space\nand tab\t.txt")
            ]
        );
        assert_eq!(
            project.exclude_path().unwrap(),
            source.canonicalize().unwrap().join(".git/info/exclude")
        );
        assert_eq!(fs::read(source.join(".git/index")).unwrap(), index_before);
        assert_eq!(git(&source, &["rev-parse", "HEAD"]), head_before);
        let worktree = root.path().join("linked");
        git(
            &source,
            &[
                "worktree",
                "add",
                "-b",
                "linked",
                worktree.to_str().unwrap(),
            ],
        );
        assert_eq!(
            runner
                .project(&worktree)
                .unwrap()
                .exclude_path()
                .unwrap()
                .canonicalize()
                .unwrap(),
            project.exclude_path().unwrap().canonicalize().unwrap()
        );
    }

    #[test]
    fn missing_git_has_stable_code_and_xcode_hint() {
        let root = tempfile::tempdir().unwrap();
        let error =
            GitRunner::with_binary(root.path().join("nonexistent-git"), Duration::from_secs(1))
                .unwrap_err();
        assert_eq!(error.code(), "E_GIT_MISSING");
        assert!(error.hint().contains("Xcode Command Line Tools"));
        assert!(error.hint().contains("xcode-select --install"));
    }

    #[cfg(unix)]
    fn executable(root: &Path, body: &str) -> PathBuf {
        use std::os::unix::fs::PermissionsExt;
        let path = root.join("git-test");
        fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o700)).unwrap();
        path
    }

    #[cfg(unix)]
    #[test]
    fn version_floor_and_typed_stderr_errors() {
        let root = tempfile::tempdir().unwrap();
        let binary = executable(root.path(), "echo 'git version 2.29.9'");
        assert_eq!(
            GitRunner::with_binary(&binary, Duration::from_secs(2))
                .unwrap_err()
                .code(),
            "E_GIT_MISSING"
        );
        for (message, code) in [
            ("Authentication failed", "E_GIT_AUTH"),
            ("Permission denied (publickey)", "E_GIT_AUTH"),
            ("Could not resolve host", "E_NETWORK"),
            (
                "xcode-select: error: invalid active developer path",
                "E_GIT_MISSING",
            ),
        ] {
            executable(
                root.path(),
                &format!(
                    "test \"$GIT_TERMINAL_PROMPT\" = 0 || exit 42\necho '{message}' >&2\nexit 128"
                ),
            );
            let error = GitRunner::with_binary(&binary, Duration::from_secs(2)).unwrap_err();
            assert_eq!(error.code(), code);
            assert_eq!(error.to_string(), message);
        }
    }

    #[cfg(unix)]
    #[test]
    fn captures_large_stdout_and_stderr_without_deadlocking() {
        let root = tempfile::tempdir().unwrap();
        let binary = executable(
            root.path(),
            "echo 'git version 2.30.0 (Apple Git)'\nhead -c 131072 /dev/zero >&2",
        );
        let runner = GitRunner::with_binary(&binary, Duration::from_secs(2)).unwrap();
        executable(
            root.path(),
            "head -c 131072 /dev/zero\nhead -c 131072 /dev/zero >&2",
        );
        assert_eq!(runner.run(None, ["ignored"]).unwrap(), vec![0; 131072]);
    }

    #[cfg(unix)]
    #[test]
    fn timeout_kills_and_reaps_git_without_waiting_for_completion() {
        let root = tempfile::tempdir().unwrap();
        let binary = executable(root.path(), "exec /bin/sleep 10");
        let start = Instant::now();
        let error = GitRunner::with_binary(binary, Duration::from_millis(50)).unwrap_err();
        assert_eq!(error.code(), "E_NETWORK");
        assert!(error.to_string().contains("timed out"));
        assert!(start.elapsed() < Duration::from_secs(2));
    }
}
