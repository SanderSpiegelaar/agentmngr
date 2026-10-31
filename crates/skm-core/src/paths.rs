use std::io;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Paths {
    pub home: PathBuf,
    pub library: PathBuf,
    pub cache: PathBuf,
    pub trash: PathBuf,
    pub logs: PathBuf,
    pub config_file: PathBuf,
    pub db_file: PathBuf,
    pub lock_file: PathBuf,
}

impl Paths {
    pub fn global_skills(user_home: &Path) -> io::Result<PathBuf> {
        Ok(std::fs::canonicalize(user_home)?.join(".agents/skills"))
    }

    /// Resolve once using explicit inputs so callers and tests need no environment mutation.
    pub fn resolve(
        cli_home: Option<&Path>,
        env_home: Option<&Path>,
        user_home: Option<&Path>,
    ) -> io::Result<Self> {
        let home = match cli_home.or(env_home) {
            Some(path) => path.to_path_buf(),
            None => user_home
                .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "user home is unavailable"))?
                .join(".skillsmgr"),
        };
        if home.as_os_str().is_empty() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "skillsmgr home must not be empty",
            ));
        }
        let home = std::path::absolute(home)?;
        Ok(Self {
            library: home.join("library"),
            cache: home.join("cache"),
            trash: home.join("trash"),
            logs: home.join("logs"),
            config_file: home.join("config.toml"),
            db_file: home.join("state.db"),
            lock_file: home.join("lock"),
            home,
        })
    }

    /// Create directories only; file contents belong to their owning modules.
    pub fn ensure_layout(&mut self) -> io::Result<()> {
        for directory in [&self.library, &self.cache, &self.trash, &self.logs] {
            std::fs::create_dir_all(directory)?;
        }
        self.home = std::fs::canonicalize(&self.home)?;
        self.library = std::fs::canonicalize(&self.library)?;
        self.cache = std::fs::canonicalize(&self.cache)?;
        self.trash = std::fs::canonicalize(&self.trash)?;
        self.logs = std::fs::canonicalize(&self.logs)?;
        self.config_file = self.home.join("config.toml");
        self.db_file = self.home.join("state.db");
        self.lock_file = self.home.join("lock");
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolution_order() {
        let root = tempfile::tempdir().unwrap();
        let cli = root.path().join("cli");
        let env = root.path().join("env");
        let user = root.path().join("user");
        assert_eq!(
            Paths::resolve(Some(&cli), Some(&env), Some(&user))
                .unwrap()
                .home,
            cli
        );
        assert_eq!(
            Paths::resolve(None, Some(&env), Some(&user)).unwrap().home,
            env
        );
        assert_eq!(
            Paths::resolve(None, None, Some(&user)).unwrap().home,
            user.join(".skillsmgr")
        );
        assert!(Paths::resolve(None, None, None).is_err());
        assert!(Paths::resolve(Some(Path::new("")), None, None).is_err());
        assert!(Paths::resolve(Some(&cli), None, None).is_ok());
    }

    #[test]
    fn first_run_layout_is_idempotent_and_preserves_contents() {
        let root = tempfile::tempdir().unwrap();
        let home = root.path().join("nested/home");
        let mut paths = Paths::resolve(Some(&home), None, None).unwrap();
        assert!(!home.exists());
        paths.ensure_layout().unwrap();
        let home = home.canonicalize().unwrap();
        for directory in [&paths.library, &paths.cache, &paths.trash, &paths.logs] {
            assert!(directory.is_dir());
        }
        let existing = paths.library.join("keep");
        std::fs::write(&existing, b"untouched").unwrap();
        paths.ensure_layout().unwrap();
        assert_eq!(std::fs::read(existing).unwrap(), b"untouched");
        assert_eq!(paths.config_file, home.join("config.toml"));
        assert_eq!(paths.db_file, home.join("state.db"));
        assert_eq!(paths.lock_file, home.join("lock"));
        assert!(!paths.db_file.exists());
        assert!(!paths.lock_file.exists());
    }
}
