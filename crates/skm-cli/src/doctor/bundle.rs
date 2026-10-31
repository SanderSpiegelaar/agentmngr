use std::{
    fs, io,
    path::{Path, PathBuf},
};

use skm_core::{Error, paths::Paths};

pub fn write(paths: &Paths, destination: &Path) -> Result<(), Error> {
    let stage = tempfile::tempdir()?;
    let mut secrets = vec![];
    let mut files = vec![];
    let mut invalid_config = false;
    for (name, path) in [
        ("config.toml", paths.config_file.clone()),
        ("library.toml", paths.library.join("library.toml")),
    ] {
        match fs::read_to_string(path) {
            Ok(text) => {
                let redacted = match toml::from_str::<toml::Value>(&text) {
                    Ok(mut value) => {
                        redact_config(&mut value, false, &mut secrets);
                        if name == "config.toml"
                            && let Some(agents) = value.get("agents")
                        {
                            files.push((
                                PathBuf::from("registry-overrides.toml"),
                                toml::to_string(agents)
                                    .map_err(|e| io::Error::other(e.to_string()))?,
                            ));
                        }
                        toml::to_string(&value).map_err(|e| io::Error::other(e.to_string()))?
                    }
                    Err(_) => {
                        invalid_config = true;
                        "# Invalid config omitted for safety.\n".into()
                    }
                };
                files.push((PathBuf::from(name), redacted));
            }
            Err(e) if e.kind() == io::ErrorKind::NotFound => {}
            Err(e) => return Err(e.into()),
        }
    }
    if paths.logs.exists() {
        for entry in fs::read_dir(&paths.logs)? {
            let entry = entry?;
            if entry.file_type()?.is_file() {
                files.push((
                    PathBuf::from("logs").join(entry.file_name()),
                    if invalid_config {
                        "Log contents omitted because config secrets could not be parsed safely.\n"
                            .into()
                    } else {
                        String::from_utf8_lossy(&fs::read(entry.path())?).into_owned()
                    },
                ));
            }
        }
    }
    secrets.sort_by_key(|value| std::cmp::Reverse(value.len()));
    for (path, mut text) in files {
        for secret in &secrets {
            if !secret.is_empty() {
                text = text.replace(secret, "[REDACTED]");
            }
        }
        text = redact_tokens(&text);
        let file = stage.path().join(path);
        fs::create_dir_all(file.parent().unwrap())?;
        fs::write(file, text)?;
    }
    fs::write(
        stage.path().join("README.txt"),
        "Support bundle. Secret config values and recognized tokens are redacted.\n",
    )?;
    let destination = std::path::absolute(destination)?;
    // zip updates existing archives, which could retain old, unredacted entries.
    let archive = tempfile::tempdir_in(destination.parent().unwrap())?;
    let staged = archive.path().join("bundle.zip");
    let output = std::process::Command::new("/usr/bin/zip")
        .current_dir(stage.path())
        .env_remove("ZIPOPT")
        .stdin(std::process::Stdio::null())
        .args(["-q", "-r"])
        .arg(&staged)
        .arg(".")
        .output()?;
    if !output.status.success() {
        return Err(io::Error::other("/usr/bin/zip failed to create support bundle").into());
    }
    fs::rename(staged, destination)?;
    Ok(())
}

fn redact_config(value: &mut toml::Value, sensitive: bool, secrets: &mut Vec<String>) {
    match value {
        toml::Value::Table(table) => {
            for (key, value) in table {
                let key = key.to_ascii_lowercase();
                redact_config(
                    value,
                    sensitive
                        || ["token", "key", "secret", "password", "auth", "env"]
                            .iter()
                            .any(|needle| key.contains(needle)),
                    secrets,
                );
            }
        }
        toml::Value::Array(values) => {
            for value in values {
                redact_config(value, sensitive, secrets);
            }
        }
        _ if sensitive => {
            secrets.push(match value {
                toml::Value::String(text) => text.clone(),
                _ => value.to_string(),
            });
            *value = toml::Value::String("[REDACTED]".into());
        }
        toml::Value::String(text) => *text = redact_tokens(text),
        _ => {}
    }
}

fn redact_tokens(text: &str) -> String {
    let mut authorization = false;
    text.split_inclusive(|c: char| {
        !c.is_ascii_alphanumeric() && !matches!(c, '_' | '-' | '.' | '/' | '+' | '=')
    })
    .map(|part| {
        let word = part.trim_end_matches(|c: char| {
            !c.is_ascii_alphanumeric() && !matches!(c, '_' | '-' | '.' | '/' | '+' | '=')
        });
        let secret = (authorization && !word.is_empty())
            || [
                "ghp_",
                "gho_",
                "ghs_",
                "ghu_",
                "ghr_",
                "github_pat_",
                "glpat-",
                "npm_",
                "ya29.",
                "sk-",
                "xoxb-",
                "xoxp-",
                "xapp-",
                "AKIA",
                "ASIA",
                "eyJ",
            ]
            .iter()
            .any(|prefix| word.contains(prefix));
        if !word.is_empty() {
            authorization =
                word.eq_ignore_ascii_case("bearer") || word.eq_ignore_ascii_case("basic");
        }
        if secret {
            format!("[REDACTED]{}", &part[word.len()..])
        } else {
            part.to_string()
        }
    })
    .collect()
}
