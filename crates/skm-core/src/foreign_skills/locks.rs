use std::{collections::BTreeMap, fs, path::Path};

use serde::Deserialize;

use crate::{
    Error,
    library::{SkillProvenance, SkillSource},
};

#[derive(Deserialize)]
struct Lock {
    version: u32,
    skills: BTreeMap<String, Entry>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Entry {
    source: String,
    source_type: String,
    source_url: Option<String>,
    #[serde(rename = "ref")]
    reference: Option<String>,
    skill_path: Option<String>,
    installed_at: Option<String>,
}

pub(crate) fn read(root: &Path, project: bool) -> Result<BTreeMap<String, SkillProvenance>, Error> {
    let path = root.join(if project {
        "skills-lock.json"
    } else {
        ".agents/.skill-lock.json"
    });
    let text = match fs::read_to_string(&path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(BTreeMap::new()),
        Err(error) => return Err(error.into()),
    };
    let lock: Lock = serde_json::from_str(&text)
        .map_err(|error| Error::ConfigParse(format!("{}: {error}", path.display())))?;
    let version = if project { 1 } else { 3 };
    if lock.version != version {
        return Err(Error::ConfigParse(format!(
            "{}: expected lock version {version}",
            path.display()
        )));
    }
    let mut result = BTreeMap::new();
    for (name, entry) in lock.skills {
        let source = match entry.source_type.as_str() {
            "github" | "git" => SkillSource::Git {
                url: entry.source_url.unwrap_or_else(|| {
                    if entry.source_type == "github" {
                        format!("https://github.com/{}", entry.source)
                    } else {
                        entry.source
                    }
                }),
                reference: entry.reference,
                path: entry
                    .skill_path
                    .map(|path| path.strip_suffix("/SKILL.md").unwrap_or(&path).to_owned()),
            },
            "local" => SkillSource::Local {
                origin: Some(if Path::new(&entry.source).is_absolute() {
                    entry.source.into()
                } else {
                    root.join(entry.source)
                }),
            },
            _ => SkillSource::Npx {
                source: entry.source,
                source_type: entry.source_type,
                url: entry.source_url,
                reference: entry.reference,
                path: entry.skill_path,
            },
        };
        result.insert(
            name,
            SkillProvenance {
                source,
                rev: None,
                base_hash: None,
                installed_at: entry
                    .installed_at
                    .map(|timestamp| timestamp.parse())
                    .transpose()
                    .map_err(|error| {
                        Error::ConfigParse(format!(
                            "{}: invalid installedAt: {error}",
                            path.display()
                        ))
                    })?,
            },
        );
    }
    Ok(result)
}
