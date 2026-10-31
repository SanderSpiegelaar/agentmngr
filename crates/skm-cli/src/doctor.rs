use std::{collections::BTreeMap, fs, io, path::Path, time::Duration};

use serde::Serialize;
use skm_core::{
    Error,
    config::Config,
    foreign_skills,
    git::GitRunner,
    paths::Paths,
    projects::{self, ProjectSkillState},
    registry::Registry,
    store::Store,
};

pub mod bundle;

#[derive(Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Status {
    Ok,
    Warn,
    Fail,
}

impl Status {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Ok => "ok",
            Self::Warn => "warn",
            Self::Fail => "fail",
        }
    }
}

#[derive(Serialize)]
pub struct Check {
    pub id: &'static str,
    pub status: Status,
    pub message: String,
    pub fix: &'static str,
}

fn check(
    id: &'static str,
    result: Result<Vec<String>, Error>,
    severity: Status,
    fix: &'static str,
) -> Check {
    match result {
        Ok(issues) if issues.is_empty() => Check {
            id,
            status: Status::Ok,
            message: "Passed".into(),
            fix,
        },
        Ok(issues) => Check {
            id,
            status: severity,
            message: issues.join("; "),
            fix,
        },
        Err(error) => Check {
            id,
            status: severity,
            message: error.to_string(),
            fix,
        },
    }
}

pub fn exit_code(checks: &[Check]) -> i32 {
    if checks.iter().any(|check| check.status == Status::Fail) {
        1
    } else if checks.iter().any(|check| check.status == Status::Warn) {
        3
    } else {
        0
    }
}

pub fn checks(paths: &mut Paths, registry: &Registry, home: &Path) -> Vec<Check> {
    let mut checks = vec![check(
        "git",
        GitRunner::new(Duration::from_secs(5)).map(|_| vec![]),
        Status::Fail,
        "Install Git >=2.30 and put it on PATH (macOS: xcode-select --install).",
    )];
    let writable = (|| {
        paths.ensure_layout()?;
        for directory in [
            &paths.home,
            &paths.library,
            &paths.cache,
            &paths.trash,
            &paths.logs,
        ] {
            tempfile::NamedTempFile::new_in(directory)?;
        }
        Ok(vec![])
    })();
    checks.push(check(
        "data-dir",
        writable,
        Status::Fail,
        "Make the data directory writable or select another directory with --home.",
    ));
    let resolved = (|| {
        let config = Config::load(&paths.config_file)?;
        for (id, overrides) in &config.agents {
            let agent = registry
                .agents
                .get(id)
                .ok_or_else(|| Error::ConfigParse(format!("unknown agent override {id}")))?;
            for (targets, overrides) in [
                (&agent.mcp, &overrides.mcp),
                (&agent.hooks, &overrides.hooks),
            ] {
                if (overrides.user.is_some() || overrides.project.is_some()) && targets.is_none() {
                    return Err(Error::ConfigParse(format!(
                        "agent {id}: override has no registry target"
                    )));
                }
            }
        }
        registry.resolve(
            home,
            &std::env::vars_os()
                .filter_map(|(key, value)| key.into_string().ok().map(|key| (key, value)))
                .collect(),
            &config,
        )
    })();
    checks.push(check(
        "registry",
        resolved
            .as_ref()
            .map(|_| vec![])
            .map_err(|e| Error::ConfigParse(e.to_string())),
        Status::Fail,
        "Fix agent IDs and relative project paths in config.toml registry overrides.",
    ));
    let scratch = tempfile::tempdir();
    let store = if paths.db_file.exists() {
        Store::open(paths)
    } else {
        scratch
            .as_ref()
            .map_err(|e| io::Error::other(e.to_string()).into())
            .and_then(|root| {
                let mut temporary = Paths::resolve(Some(root.path()), None, None)?;
                temporary.ensure_layout()?;
                Store::open(&temporary)
            })
    };
    let projects = store
        .as_ref()
        .map_err(|e| Error::ConfigParse(e.to_string()))
        .and_then(projects::list);
    let roots: Vec<_> = std::iter::once(home.to_path_buf())
        .chain(
            projects
                .as_ref()
                .into_iter()
                .flatten()
                .map(|p| p.path.clone()),
        )
        .collect();
    let mut skill_roots: Vec<_> = roots
        .iter()
        .map(|root| root.join(".agents/skills"))
        .collect();
    if let Ok(registry) = &resolved {
        for agent in registry.agents.values() {
            skill_roots.push(agent.skills.user.clone());
            for root in roots.iter().skip(1) {
                skill_roots.push(root.join(&agent.skills.project));
            }
        }
    }
    skill_roots.sort();
    skill_roots.dedup();
    let links = (|| {
        let mut issues = vec![];
        for root in &skill_roots {
            broken_links(root, &mut issues)?;
        }
        Ok(issues)
    })();
    checks.push(check(
        "broken-links", links, Status::Warn,
        "Restore missing link destinations or remove the broken skill links, then run skm on again.",
    ));
    checks.push(check(
        "agent-configs",
        (|| {
            let registry = resolved.as_ref().map_err(|_| {
                Error::ConfigParse(
                    "registry overrides must be fixed before checking agent configs".into(),
                )
            })?;
            let mut files = BTreeMap::new();
            for agent in registry.agents.values() {
                for targets in [&agent.mcp, &agent.hooks].into_iter().flatten() {
                    if let Some(target) = &targets.user {
                        files.insert(
                            target.file.clone(),
                            target.format.as_deref().unwrap_or("json"),
                        );
                    }
                    if let Some(target) = &targets.project {
                        for root in roots.iter().skip(1) {
                            files.insert(
                                root.join(&target.file),
                                target.format.as_deref().unwrap_or("json"),
                            );
                        }
                    }
                }
            }
            let mut issues = vec![];
            for (file, format) in files {
                let text = match fs::read_to_string(&file) {
                    Ok(text) => text,
                    Err(e) if e.kind() == io::ErrorKind::NotFound => continue,
                    Err(e) => {
                        issues.push(format!("{}: {e}", file.display()));
                        continue;
                    }
                };
                let valid = if format == "toml" {
                    toml::from_str::<toml::Value>(&text).is_ok()
                } else {
                    serde_json::from_str::<serde_json::Value>(&if format == "jsonc" {
                        jsonc(&text)
                    } else {
                        text
                    })
                    .is_ok()
                };
                if !valid {
                    issues.push(format!("{}: invalid {format}", file.display()));
                }
            }
            Ok(issues)
        })(),
        Status::Fail,
        "Fix the syntax in the listed agent config files; doctor does not modify them.",
    ));
    let foreign = (|| {
        let store = store
            .as_ref()
            .map_err(|e| Error::ConfigParse(e.to_string()))?;
        let mut issues = vec![];
        for (index, root) in roots.iter().enumerate() {
            for skill in foreign_skills::list_foreign(
                paths,
                store,
                home,
                (index > 0).then_some(root.as_path()),
            )? {
                issues.push(skill.target.display().to_string());
            }
        }
        Ok(issues)
    })();
    checks.push(check(
        "foreign-conflicts", foreign, Status::Warn,
        "Review unmanaged skills; use skm adopt or move conflicting entries aside before enabling skills.",
    ));
    checks.push(check(
        "edited-projects",
        (|| {
            let store = store
                .as_ref()
                .map_err(|e| Error::ConfigParse(e.to_string()))?;
            let projects = projects
                .as_ref()
                .map_err(|e| Error::ConfigParse(e.to_string()))?;
            let mut issues = vec![];
            for project in projects {
                let (registration, _) = projects::plan_registration(store, &project.path)?;
                for (name, state) in registration.reattachment.skills {
                    if state == ProjectSkillState::Edited {
                        issues.push(
                            project
                                .path
                                .join(".agents/skills")
                                .join(name)
                                .display()
                                .to_string(),
                        );
                    }
                }
            }
            Ok(issues)
        })(),
        Status::Warn,
        "Review Edited project skills and preserve local changes before running skm project resume <path>.",
    ));
    #[cfg(target_os = "linux")]
    checks.push(inotify_check(Path::new(
        "/proc/sys/fs/inotify/max_user_watches",
    )));
    if let Some(check) = secrets_scan(paths) {
        checks.push(check);
    }
    checks
}

// M3/M5 supplies the Keychain-backed scanner here; no scan is claimed before then.
fn secrets_scan(_paths: &Paths) -> Option<Check> {
    None
}

#[cfg(any(target_os = "linux", test))]
fn inotify_check(path: &Path) -> Check {
    let result = (|| {
        let value: u64 = fs::read_to_string(path)?
            .trim()
            .parse()
            .map_err(|_| Error::ConfigParse("invalid inotify max_user_watches".into()))?;
        Ok::<_, Error>(value)
    })();
    let mut check = check(
        "inotify",
        result
            .as_ref()
            .map(|_| vec![])
            .map_err(|e| Error::ConfigParse(e.to_string())),
        Status::Warn,
        "Raise fs.inotify.max_user_watches to 524288 with sysctl and persist it in /etc/sysctl.d/.",
    );
    if let Ok(value) = result {
        check.message = format!("max_user_watches={value} (recommended >=524288)");
        check.status = if value < 524_288 {
            Status::Warn
        } else {
            Status::Ok
        };
    }
    check
}

fn broken_links(path: &Path, issues: &mut Vec<String>) -> io::Result<()> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(e),
    };
    if metadata.is_symlink() {
        if fs::metadata(path).is_err() {
            issues.push(path.display().to_string());
        }
    } else if metadata.is_dir() {
        for entry in fs::read_dir(path)? {
            broken_links(&entry?.path(), issues)?;
        }
    }
    Ok(())
}

fn jsonc(text: &str) -> String {
    let mut chars = text.chars().peekable();
    let mut output = String::new();
    let (mut quoted, mut escaped) = (false, false);
    while let Some(c) = chars.next() {
        if quoted {
            output.push(c);
            if escaped {
                escaped = false;
            } else if c == '\\' {
                escaped = true;
            } else if c == '"' {
                quoted = false;
            }
        } else if c == '"' {
            quoted = true;
            output.push(c);
        } else if c == '/' && chars.peek() == Some(&'/') {
            for c in chars.by_ref() {
                if c == '\n' {
                    output.push('\n');
                    break;
                }
            }
        } else if c == '/' && chars.peek() == Some(&'*') {
            chars.next();
            let mut closed = false;
            while let Some(c) = chars.next() {
                if c == '*' && chars.peek() == Some(&'/') {
                    chars.next();
                    closed = true;
                    break;
                }
            }
            if !closed {
                return String::new();
            }
            output.push(' ');
        } else {
            output.push(c);
        }
    }
    let mut chars = output.chars().peekable();
    let mut result = String::new();
    let (mut quoted, mut escaped) = (false, false);
    while let Some(c) = chars.next() {
        if quoted {
            result.push(c);
            if escaped {
                escaped = false;
            } else if c == '\\' {
                escaped = true;
            } else if c == '"' {
                quoted = false;
            }
        } else if c == '"' {
            quoted = true;
            result.push(c);
        } else if c != ','
            || !chars
                .clone()
                .find(|c| !c.is_whitespace())
                .is_some_and(|c| c == '}' || c == ']')
        {
            result.push(c);
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inotify_limit_fixture_reports_value_and_sysctl_hint() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("max_user_watches");
        for (value, status) in [
            ("8192", Status::Warn),
            ("524288", Status::Ok),
            ("invalid", Status::Warn),
        ] {
            fs::write(&path, value).unwrap();
            let result = inotify_check(&path);
            assert!(result.status == status);
            assert!(result.fix.contains("fs.inotify.max_user_watches to 524288"));
            assert!(result.fix.contains("sysctl"));
            if value != "invalid" {
                assert!(result.message.contains(value));
            }
        }
        fs::remove_file(&path).unwrap();
        assert!(inotify_check(&path).status == Status::Warn);
    }
}
