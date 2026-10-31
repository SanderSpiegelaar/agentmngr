use std::collections::BTreeMap;
use std::fs::File;
use std::io::{BufRead, BufReader, Read};
use std::path::Path;

use serde_yaml_ng::{Mapping, Value};

use crate::Error;

pub const MAX_FRONTMATTER_BYTES: usize = 64 * 1024;

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Frontmatter {
    pub name: Option<String>,
    pub description: Option<String>,
    pub license: Option<String>,
    pub compatibility: Option<String>,
    pub metadata: BTreeMap<String, String>,
    pub allowed_tools: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum Warning {
    #[error("Missing SKILL.md")]
    MissingSkillMd,
    #[error("Missing opening frontmatter delimiter ---")]
    MissingFrontmatter,
    #[error("Missing closing frontmatter delimiter ---")]
    UnclosedFrontmatter,
    #[error("Frontmatter exceeds the 64 KiB limit")]
    FrontmatterTooLarge,
    #[error("Invalid YAML frontmatter: {0}")]
    InvalidYaml(String),
    #[error("Frontmatter field '{0}' has an invalid type")]
    InvalidField(String),
    #[error("Name must contain 1-64 characters")]
    NameLength,
    #[error("Name must use lowercase alphanumeric characters and single, non-edge hyphens")]
    NameCharacters,
    #[error("Skill name '{name}' does not match directory '{directory}'")]
    NameDirectoryMismatch { name: String, directory: String },
    #[error("Description must contain 1-1024 characters and must not be blank")]
    DescriptionLength,
    #[error("Compatibility must contain 1-500 characters when provided")]
    CompatibilityLength,
}

pub fn read_skill(directory: &Path) -> Result<(Frontmatter, Vec<Warning>), Error> {
    let file = match File::open(directory.join("SKILL.md")) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok((Frontmatter::default(), vec![Warning::MissingSkillMd]));
        }
        Err(error) => return Err(error.into()),
    };
    // A larger file buffer could prefetch body bytes past the closing delimiter.
    read_frontmatter(
        &mut BufReader::with_capacity(1, file),
        &directory.file_name().unwrap_or_default().to_string_lossy(),
    )
}

/// Consumes only through the closing delimiter, or at most 64 KiB.
/// Malformed metadata produces warnings; only reader failures return an error.
pub fn read_frontmatter(
    reader: &mut impl BufRead,
    directory_name: &str,
) -> Result<(Frontmatter, Vec<Warning>), Error> {
    let mut yaml = Vec::new();
    let mut line = Vec::new();
    let mut consumed = 0;
    loop {
        line.clear();
        let count = reader
            .take((MAX_FRONTMATTER_BYTES - consumed) as u64)
            .read_until(b'\n', &mut line)?;
        if count == 0 {
            let warning = if consumed == MAX_FRONTMATTER_BYTES {
                Warning::FrontmatterTooLarge
            } else if consumed == 0 {
                Warning::MissingFrontmatter
            } else {
                Warning::UnclosedFrontmatter
            };
            return Ok((Frontmatter::default(), vec![warning]));
        }
        let delimiter = line.strip_suffix(b"\n").unwrap_or(&line);
        let delimiter = delimiter.strip_suffix(b"\r").unwrap_or(delimiter) == b"---";
        if consumed == 0 && !delimiter {
            return Ok((Frontmatter::default(), vec![Warning::MissingFrontmatter]));
        }
        let first_line = consumed == 0;
        consumed += count;
        if consumed == MAX_FRONTMATTER_BYTES && !line.ends_with(b"\n") {
            return Ok((Frontmatter::default(), vec![Warning::FrontmatterTooLarge]));
        }
        if delimiter && !first_line {
            break;
        }
        if !first_line {
            yaml.extend_from_slice(&line);
        }
    }

    let fields = match serde_yaml_ng::from_slice::<Mapping>(&yaml) {
        Ok(fields) => fields,
        Err(error) => {
            return Ok((
                Frontmatter::default(),
                vec![Warning::InvalidYaml(error.to_string())],
            ));
        }
    };
    let mut warnings = Vec::new();
    let mut string = |key: &str| match fields.get(Value::from(key)) {
        None => None,
        Some(Value::String(value)) => Some(value.clone()),
        Some(_) => {
            warnings.push(Warning::InvalidField(key.into()));
            None
        }
    };
    let mut frontmatter = Frontmatter {
        name: string("name"),
        description: string("description"),
        license: string("license"),
        compatibility: string("compatibility"),
        allowed_tools: string("allowed-tools"),
        ..Frontmatter::default()
    };
    if let Some(metadata) = fields.get(Value::from("metadata")) {
        match metadata {
            Value::Mapping(values) => {
                for (key, value) in values {
                    if let (Some(key), Some(value)) = (key.as_str(), value.as_str()) {
                        frontmatter.metadata.insert(key.into(), value.into());
                    } else {
                        warnings.push(Warning::InvalidField("metadata".into()));
                    }
                }
            }
            _ => warnings.push(Warning::InvalidField("metadata".into())),
        }
    }
    warnings.extend(frontmatter.validate(directory_name));
    Ok((frontmatter, warnings))
}

impl Frontmatter {
    pub fn validate(&self, directory_name: &str) -> Vec<Warning> {
        let mut warnings = Vec::new();
        let name = self.name.as_deref().unwrap_or_default();
        if !(1..=64).contains(&name.chars().count()) {
            warnings.push(Warning::NameLength);
        }
        if name.starts_with('-')
            || name.ends_with('-')
            || name.contains("--")
            || !name
                .chars()
                .all(|c| c == '-' || (c.is_alphanumeric() && !c.is_uppercase()))
        {
            warnings.push(Warning::NameCharacters);
        }
        if self.name.is_some() && name != directory_name {
            warnings.push(Warning::NameDirectoryMismatch {
                name: name.into(),
                directory: directory_name.into(),
            });
        }
        let description = self.description.as_deref().unwrap_or_default();
        if description.trim().is_empty() || description.chars().count() > 1024 {
            warnings.push(Warning::DescriptionLength);
        }
        if let Some(compatibility) = &self.compatibility
            && (compatibility.is_empty() || compatibility.chars().count() > 500)
        {
            warnings.push(Warning::CompatibilityLength);
        }
        warnings
    }
}
