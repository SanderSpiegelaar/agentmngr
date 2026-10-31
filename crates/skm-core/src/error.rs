#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("{0}")]
    ForeignTarget(String),
    #[error("{0}")]
    ConfigParse(String),
    #[error("{0}")]
    ConfigChanged(String),
    #[error("{0}")]
    GitAuth(String),
    #[error("{0}")]
    GitMissing(String),
    #[error("{0}")]
    Network(String),
    #[error("{0}")]
    Locked(String),
    #[error("{0}")]
    InvalidSkill(String),
    #[error("{0}")]
    NameCollision(String),
    #[error("{0}")]
    SecretMissing(String),
    #[error("{0}")]
    SecretInProject(String),
    #[error("{0}")]
    ProjectEdited(String),
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error("{0}")]
    Logging(String),
    #[error("{0}")]
    Database(String),
}

impl Error {
    pub const fn code(&self) -> &'static str {
        match self {
            Self::ForeignTarget(_) => "E_FOREIGN_TARGET",
            Self::ConfigParse(_) => "E_CONFIG_PARSE",
            Self::ConfigChanged(_) => "E_CONFIG_CHANGED",
            Self::GitAuth(_) => "E_GIT_AUTH",
            Self::GitMissing(_) => "E_GIT_MISSING",
            Self::Network(_) => "E_NETWORK",
            Self::Locked(_) => "E_LOCKED",
            Self::InvalidSkill(_) => "E_INVALID_SKILL",
            Self::NameCollision(_) => "E_NAME_COLLISION",
            Self::SecretMissing(_) => "E_SECRET_MISSING",
            Self::SecretInProject(_) => "E_SECRET_IN_PROJECT",
            Self::ProjectEdited(_) => "E_PROJECT_EDITED",
            Self::Io(_) => "E_IO",
            Self::Logging(_) => "E_LOGGING",
            Self::Database(_) => "E_DATABASE",
        }
    }

    pub const fn hint(&self) -> &'static str {
        match self {
            Self::ForeignTarget(_) => "Move the unmanaged target aside or explicitly adopt it.",
            Self::ConfigParse(_) => {
                "Fix the config syntax before retrying; the file was not changed."
            }
            Self::ConfigChanged(_) => "Reload the config and create a new plan before applying.",
            Self::GitAuth(_) => "Check your Git credentials and repository access.",
            Self::GitMissing(_) => {
                "Install Git >=2.30 with Xcode Command Line Tools (`xcode-select --install`) and make it available on PATH."
            }
            Self::Network(_) => "Check your connection and retry.",
            Self::Locked(_) => "Wait for the other apply operation to finish and retry.",
            Self::InvalidSkill(_) => "Fix the skill metadata or contents and retry.",
            Self::NameCollision(_) => "Rename the conflicting item or choose a different target.",
            Self::SecretMissing(_) => "Supply the required secret or environment variable.",
            Self::SecretInProject(_) => {
                "Supply the secret through an environment variable; do not write it into the project."
            }
            Self::ProjectEdited(_) => {
                "Review the project edits and create a new plan before applying."
            }
            Self::Io(_) => "Check the path, permissions, and available disk space.",
            Self::Logging(_) => "Check the logs directory and initialize logging only once.",
            Self::Database(_) => {
                "Check the database path and schema version, then reopen the store."
            }
        }
    }
}

impl From<rusqlite::Error> for Error {
    fn from(error: rusqlite::Error) -> Self {
        Self::Database(error.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::Error;

    #[test]
    fn stable_codes_and_documented_hints() {
        let cases = [
            (Error::ForeignTarget("target".into()), "E_FOREIGN_TARGET"),
            (Error::ConfigParse("config".into()), "E_CONFIG_PARSE"),
            (Error::ConfigChanged("config".into()), "E_CONFIG_CHANGED"),
            (Error::GitAuth("git".into()), "E_GIT_AUTH"),
            (Error::GitMissing("git".into()), "E_GIT_MISSING"),
            (Error::Network("network".into()), "E_NETWORK"),
            (Error::Locked("lock".into()), "E_LOCKED"),
            (Error::InvalidSkill("skill".into()), "E_INVALID_SKILL"),
            (Error::NameCollision("name".into()), "E_NAME_COLLISION"),
            (Error::SecretMissing("secret".into()), "E_SECRET_MISSING"),
            (
                Error::SecretInProject("secret".into()),
                "E_SECRET_IN_PROJECT",
            ),
            (Error::ProjectEdited("project".into()), "E_PROJECT_EDITED"),
            (std::io::Error::other("disk").into(), "E_IO"),
            (Error::Logging("logger".into()), "E_LOGGING"),
            (Error::Database("database".into()), "E_DATABASE"),
        ];
        let docs = include_str!("../../../docs/errors.md");
        for (error, code) in cases {
            assert_eq!(error.code(), code);
            assert!(!error.to_string().is_empty());
            assert!(!error.hint().is_empty());
            assert!(docs.contains(&format!("| `{code}` |")), "missing {code}");
            assert!(docs.contains(error.hint()), "missing hint for {code}");
        }
    }
}
