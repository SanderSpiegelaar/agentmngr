use std::path::PathBuf;

use crate::Error;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Source {
    GitHub {
        owner: String,
        repo: String,
        skill: Option<String>,
    },
    Git {
        url: String,
        reference: Option<String>,
        path: Option<String>,
    },
    Local {
        path: PathBuf,
    },
    SkillsSh {
        owner: String,
        repo: String,
        skill: String,
    },
}

/// Parses without filesystem or network access. `path` is the separate CLI `--path` value.
pub fn parse(spec: &str, path: Option<&str>) -> Result<Source, Error> {
    if spec.is_empty() || spec.chars().any(char::is_control) {
        return Err(invalid(
            "source must be nonempty and contain no control characters",
        ));
    }
    if spec.starts_with('/')
        || spec.starts_with("./")
        || spec.starts_with("../")
        || matches!(spec, "." | "..")
    {
        without_path(path)?;
        return Ok(Source::Local { path: spec.into() });
    }
    if spec.chars().any(char::is_whitespace) {
        return Err(invalid(
            "source contains whitespace; prefix local paths with ./",
        ));
    }
    if let Some(value) = path {
        validate_path(value)?;
    }
    let skills = spec
        .strip_prefix("https://skills.sh/")
        .or_else(|| spec.strip_prefix("skills.sh/"));
    if let Some(skills) = skills {
        without_path(path)?;
        let parts: Vec<_> = skills.trim_end_matches('/').split('/').collect();
        if let [owner, repo, skill] = parts.as_slice()
            && [owner, repo, skill].into_iter().all(|part| name(part))
        {
            return Ok(Source::SkillsSh {
                owner: (*owner).into(),
                repo: (*repo).into(),
                skill: (*skill).into(),
            });
        }
        return Err(invalid(
            "skills.sh source must be skills.sh/owner/repo/skill",
        ));
    }
    if spec.contains("://") || spec.contains(':') {
        let (url, reference) = match spec.split_once('#') {
            Some((url, reference)) => {
                validate_reference(reference)?;
                (url, Some(reference.to_owned()))
            }
            None => (spec, None),
        };
        validate_git_url(url)?;
        if let Some(tree) = url.strip_prefix("https://github.com/") {
            let parts: Vec<_> = tree.trim_end_matches('/').split('/').collect();
            if parts.get(2) == Some(&"tree") {
                if reference.is_some() || path.is_some() {
                    return Err(invalid(
                        "GitHub tree URL conflicts with #ref or --path; use a repository URL instead",
                    ));
                }
                if parts.len() < 4 || !name(parts[0]) || !name(parts[1]) {
                    return Err(invalid("GitHub tree URL needs owner/repo/tree/ref[/path]"));
                }
                validate_reference(parts[3])?;
                let subpath = (parts.len() > 4).then(|| parts[4..].join("/"));
                if let Some(value) = &subpath {
                    validate_path(value)?;
                }
                return Ok(Source::Git {
                    url: format!("https://github.com/{}/{}.git", parts[0], parts[1]),
                    reference: Some(parts[3].into()),
                    path: subpath,
                });
            }
            if parts.len() != 2 || !name(parts[0]) || !name(parts[1]) {
                return Err(invalid(
                    "ambiguous GitHub URL; use owner/repo, a tree URL, or a repository URL with --path",
                ));
            }
        }
        if url.contains("/-/tree/") || url.contains("/-/blob/") {
            return Err(invalid(
                "use the git repository URL with #ref and --path, not a GitLab browsing URL",
            ));
        }
        return Ok(Source::Git {
            url: url.into(),
            reference,
            path: path.map(str::to_owned),
        });
    }
    without_path(path)?;
    let (repository, skill) = match spec.split_once('@') {
        Some((repository, skill)) if name(skill) => (repository, Some(skill.to_owned())),
        Some(_) => return Err(invalid("@skill must select one nonempty skill name")),
        None => (spec, None),
    };
    if let Some((owner, repo)) = repository.split_once('/')
        && name(owner)
        && name(repo)
        && !owner.contains('.')
    {
        return Ok(Source::GitHub {
            owner: owner.into(),
            repo: repo.into(),
            skill,
        });
    }
    Err(invalid(
        "ambiguous source; use owner/repo[@skill], an explicit git URL, or ./path for a local folder",
    ))
}

fn invalid(message: &str) -> Error {
    Error::ConfigParse(format!("invalid source: {message}"))
}

fn without_path(path: Option<&str>) -> Result<(), Error> {
    if path.is_some() {
        return Err(invalid("--path requires a git repository URL"));
    }
    Ok(())
}

fn name(value: &str) -> bool {
    !value.is_empty()
        && !matches!(value, "." | "..")
        && value
            .chars()
            .all(|c| c.is_alphanumeric() || matches!(c, '-' | '_' | '.'))
}

fn validate_path(path: &str) -> Result<(), Error> {
    if path.contains('\\')
        || path.chars().any(char::is_control)
        || path.split('/').any(|part| matches!(part, "" | "." | ".."))
    {
        return Err(invalid(
            "--path or tree subpath must be a nonempty relative path without . or .. components",
        ));
    }
    Ok(())
}

fn validate_reference(reference: &str) -> Result<(), Error> {
    if reference.is_empty()
        || reference.starts_with(['-', '/'])
        || reference.ends_with(['/', '.'])
        || reference.contains(['#', '~', '^', ':', '?', '*', '[', '\\'])
        || reference.contains("..")
        || reference.contains("@{")
        || reference
            .split('/')
            .any(|part| part.is_empty() || part.starts_with('.') || part.ends_with(".lock"))
        || reference.chars().any(char::is_whitespace)
    {
        return Err(invalid(
            "#ref must be a nonempty git ref without spaces or revision operators",
        ));
    }
    Ok(())
}

fn validate_git_url(url: &str) -> Result<(), Error> {
    if url.contains(['?', '#', '\\']) {
        return Err(invalid(
            "git URL must not contain a query, extra fragment, or backslash",
        ));
    }
    if let Some((scheme, address)) = url.split_once("://") {
        if !matches!(scheme, "https" | "http" | "ssh" | "git" | "file") {
            return Err(invalid(
                "unsupported git URL scheme; use https, http, ssh, git, or file",
            ));
        }
        if scheme == "file" {
            if address.starts_with('/') && address.len() > 1 {
                return Ok(());
            }
        } else if let Some((host, repository)) = address.split_once('/')
            && !host.is_empty()
            && !host.ends_with('@')
            && !repository.trim_matches('/').is_empty()
        {
            return Ok(());
        }
    } else if let Some((host, repository)) = url.split_once(':')
        && let Some((user, host)) = host.split_once('@')
        && !user.is_empty()
        && !host.is_empty()
        && !host.contains('/')
        && !repository.is_empty()
    {
        return Ok(());
    }
    Err(invalid(
        "git URL needs a host and repository path; use ssh://host/path or user@host:path for SSH",
    ))
}

#[cfg(test)]
mod tests {
    use super::{Source, parse};

    fn github(owner: &str, repo: &str, skill: Option<&str>) -> Source {
        Source::GitHub {
            owner: owner.into(),
            repo: repo.into(),
            skill: skill.map(str::to_owned),
        }
    }

    fn git(url: &str, reference: Option<&str>, path: Option<&str>) -> Source {
        Source::Git {
            url: url.into(),
            reference: reference.map(str::to_owned),
            path: path.map(str::to_owned),
        }
    }

    fn local(path: &str) -> Source {
        Source::Local { path: path.into() }
    }

    fn skills_sh() -> Source {
        Source::SkillsSh {
            owner: "vercel-labs".into(),
            repo: "skills".into(),
            skill: "find-skills".into(),
        }
    }

    #[test]
    fn parses_source_specs() {
        let cases = [
            ("owner/repo", None, github("owner", "repo", None)),
            (
                "vercel-labs/skills",
                None,
                github("vercel-labs", "skills", None),
            ),
            (
                "owner/repo@skill",
                None,
                github("owner", "repo", Some("skill")),
            ),
            (
                "owner/repo@技能",
                None,
                github("owner", "repo", Some("技能")),
            ),
            ("owner/repo.git", None, github("owner", "repo.git", None)),
            ("./skills", None, local("./skills")),
            ("../skills", None, local("../skills")),
            ("/opt/skills", None, local("/opt/skills")),
            ("./local skills", None, local("./local skills")),
            (
                "./owner/repo@skill#ref",
                None,
                local("./owner/repo@skill#ref"),
            ),
            (".", None, local(".")),
            ("..", None, local("..")),
            (
                "skills.sh/vercel-labs/skills/find-skills",
                None,
                skills_sh(),
            ),
            (
                "https://skills.sh/vercel-labs/skills/find-skills",
                None,
                skills_sh(),
            ),
            (
                "https://skills.sh/vercel-labs/skills/find-skills/",
                None,
                skills_sh(),
            ),
            (
                "https://github.com/owner/repo",
                None,
                git("https://github.com/owner/repo", None, None),
            ),
            (
                "https://github.com/owner/repo.git#v1",
                None,
                git("https://github.com/owner/repo.git", Some("v1"), None),
            ),
            (
                "https://github.com/owner/repo/tree/main/skills/foo",
                None,
                git(
                    "https://github.com/owner/repo.git",
                    Some("main"),
                    Some("skills/foo"),
                ),
            ),
            (
                "https://github.com/owner/repo/tree/main",
                None,
                git("https://github.com/owner/repo.git", Some("main"), None),
            ),
            (
                "https://gitlab.com/group/subgroup/repo.git#feature/topic",
                Some("skills/foo"),
                git(
                    "https://gitlab.com/group/subgroup/repo.git",
                    Some("feature/topic"),
                    Some("skills/foo"),
                ),
            ),
            (
                "https://gitlab.com/group/repo",
                None,
                git("https://gitlab.com/group/repo", None, None),
            ),
            (
                "git@gitlab.com:group/repo.git#main",
                None,
                git("git@gitlab.com:group/repo.git", Some("main"), None),
            ),
            (
                "git@github.com:owner/repo.git",
                Some("skills"),
                git("git@github.com:owner/repo.git", None, Some("skills")),
            ),
            (
                "ssh://git@gitlab.com:2222/group/repo.git#v2",
                None,
                git("ssh://git@gitlab.com:2222/group/repo.git", Some("v2"), None),
            ),
            (
                "ssh://git.example/repo",
                None,
                git("ssh://git.example/repo", None, None),
            ),
            (
                "git://git.example/repo.git",
                None,
                git("git://git.example/repo.git", None, None),
            ),
            (
                "http://localhost/repo.git",
                None,
                git("http://localhost/repo.git", None, None),
            ),
            (
                "file:///opt/repos/skills.git#HEAD",
                Some("skill folder"),
                git(
                    "file:///opt/repos/skills.git",
                    Some("HEAD"),
                    Some("skill folder"),
                ),
            ),
        ];
        for (spec, path, expected) in cases {
            assert_eq!(parse(spec, path).unwrap(), expected, "{spec} {path:?}");
        }
    }

    #[test]
    fn rejects_ambiguous_and_invalid_sources() {
        let cases = [
            ("", None, "nonempty"),
            ("repo", None, "./path"),
            ("owner/repo/skill", None, "ambiguous"),
            ("github.com/owner/repo", None, "explicit git URL"),
            ("gitlab.com/repo", None, "ambiguous"),
            ("~/skills", None, "./path"),
            (" owner/repo", None, "whitespace"),
            ("owner/repo\n", None, "control"),
            ("owner/repo@", None, "@skill"),
            ("owner/repo@skill@other", None, "@skill"),
            ("owner/..", None, "ambiguous"),
            ("skills.sh/owner/repo", None, "owner/repo/skill"),
            (
                "https://skills.sh/owner/repo/skill?x=1",
                None,
                "owner/repo/skill",
            ),
            (
                "https://skills.sh/owner/repo/skill/extra",
                None,
                "owner/repo/skill",
            ),
            ("owner/repo", Some("skills"), "requires a git"),
            ("./skills", Some("nested"), "requires a git"),
            (
                "skills.sh/owner/repo/skill",
                Some("nested"),
                "requires a git",
            ),
            ("https://github.com/o/r/tree/main/a", Some("b"), "conflicts"),
            ("https://github.com/o/r/tree/main/a#v1", None, "conflicts"),
            (
                "https://github.com/o/r/tree",
                None,
                "needs owner/repo/tree/ref",
            ),
            (
                "https://github.com/o/r/blob/main/SKILL.md",
                None,
                "ambiguous",
            ),
            (
                "https://gitlab.com/g/r/-/tree/main/skill",
                None,
                "GitLab browsing",
            ),
            ("https://git.example/r.git#", None, "#ref"),
            ("https://git.example/r.git#main#other", None, "#ref"),
            ("https://git.example/r.git#../main", None, "#ref"),
            ("https://git.example/r.git#main.lock", None, "#ref"),
            ("https://git.example/r.git?tab=readme", None, "query"),
            ("https:///repo.git", None, "host and repository"),
            ("https://git.example", None, "host and repository"),
            ("ssh://git@/repo", None, "host and repository"),
            ("git@host:", None, "host and repository"),
            ("host:repo", None, "ssh://host/path"),
            ("ftp://host/repo", None, "unsupported"),
            ("file://relative/repo", None, "host and repository"),
            ("https://git.example/r", Some(""), "relative path"),
            ("https://git.example/r", Some("../escape"), "relative path"),
            ("https://git.example/r", Some("/absolute"), "relative path"),
            ("https://git.example/r", Some("a/./b"), "relative path"),
            ("https://git.example/r", Some("a/../b"), "relative path"),
            ("https://git.example/r", Some("a\\b"), "relative path"),
            ("https://git.example/r", Some("a\nb"), "relative path"),
        ];
        for (spec, path, hint) in cases {
            let error = parse(spec, path).unwrap_err();
            assert_eq!(error.code(), "E_CONFIG_PARSE", "{spec} {path:?}");
            assert!(error.to_string().contains(hint), "{spec} {path:?}: {error}");
        }
    }
}
