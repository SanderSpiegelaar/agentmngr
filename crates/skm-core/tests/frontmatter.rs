use std::io::{self, BufRead, Cursor, Read};
use std::time::{Duration, Instant};

use skm_core::frontmatter::{
    Frontmatter, MAX_FRONTMATTER_BYTES, Warning, read_frontmatter, read_skill,
};

const VALID: &[u8] = include_bytes!("../../../tests/fixtures/frontmatter/valid.md");
const INVALID: &[u8] = include_bytes!("../../../tests/fixtures/frontmatter/invalid.md");
const MISSING: &[u8] = include_bytes!("../../../tests/fixtures/frontmatter/missing.md");

#[test]
fn valid_fixture_preserves_spec_fields_and_ignores_unknown_keys() {
    let mut reader = Cursor::new(VALID);
    let (frontmatter, warnings) = read_frontmatter(&mut reader, "valid-skill").unwrap();
    assert!(warnings.is_empty(), "{warnings:?}");
    assert_eq!(frontmatter.name.as_deref(), Some("valid-skill"));
    assert_eq!(
        frontmatter.description.as_deref(),
        Some("Extract data when processing reports.")
    );
    assert_eq!(frontmatter.license.as_deref(), Some("MIT"));
    assert_eq!(frontmatter.compatibility.as_deref(), Some("Requires git"));
    assert_eq!(frontmatter.metadata["author"], "example");
    assert_eq!(
        frontmatter.allowed_tools.as_deref(),
        Some("Read Bash(git:*)")
    );
    assert!(VALID[reader.position() as usize..].starts_with(b"Body"));
}

#[test]
fn invalid_fixture_returns_metadata_and_warnings_not_an_error() {
    let (frontmatter, warnings) = read_frontmatter(&mut Cursor::new(INVALID), "other").unwrap();
    assert_eq!(frontmatter.name.as_deref(), Some("Bad--Name-"));
    assert!(warnings.contains(&Warning::NameCharacters));
    assert!(warnings.contains(&Warning::DescriptionLength));
    assert!(
        warnings
            .iter()
            .any(|w| matches!(w, Warning::NameDirectoryMismatch { .. }))
    );
    assert!(
        warnings
            .iter()
            .all(|warning| !warning.to_string().is_empty())
    );
}

#[test]
fn missing_and_malformed_frontmatter_are_warnings() {
    for bytes in [MISSING, b"".as_slice()] {
        let (_, warnings) = read_frontmatter(&mut Cursor::new(bytes), "skill").unwrap();
        assert_eq!(warnings, [Warning::MissingFrontmatter]);
    }
    for bytes in [b"---\nname: [\n---\n".as_slice(), b"---\n- list\n---\n"] {
        let (_, warnings) = read_frontmatter(&mut Cursor::new(bytes), "skill").unwrap();
        assert!(matches!(warnings.as_slice(), [Warning::InvalidYaml(_)]));
    }
    let (_, warnings) = read_frontmatter(&mut Cursor::new(b"---\nname: skill\n"), "skill").unwrap();
    assert_eq!(warnings, [Warning::UnclosedFrontmatter]);
}

#[test]
fn huge_files_stop_at_cap_even_for_a_single_line() {
    for prefix in ["---\n", "---\n# comment\n"] {
        let bytes = format!(
            "{prefix}{}\n---\nbody",
            "x".repeat(MAX_FRONTMATTER_BYTES * 2)
        );
        let mut reader = Cursor::new(bytes);
        let (_, warnings) = read_frontmatter(&mut reader, "skill").unwrap();
        assert_eq!(warnings, [Warning::FrontmatterTooLarge]);
        assert_eq!(reader.position(), MAX_FRONTMATTER_BYTES as u64);
    }
    let mut bytes = VALID.to_vec();
    bytes.extend(std::iter::repeat_n(b'x', MAX_FRONTMATTER_BYTES * 2));
    let mut reader = Cursor::new(bytes);
    let (_, warnings) = read_frontmatter(&mut reader, "valid-skill").unwrap();
    assert!(warnings.is_empty());
    assert!(reader.position() < 1024);
}

#[test]
fn cap_includes_delimiters_and_does_not_accept_a_truncated_delimiter() {
    let prefix = "---\nname: skill\ndescription: good\n#";
    let bytes = format!(
        "{prefix}{}\n---\nbody",
        "x".repeat(MAX_FRONTMATTER_BYTES - prefix.len() - 5)
    );
    let mut reader = Cursor::new(bytes);
    assert!(read_frontmatter(&mut reader, "skill").unwrap().1.is_empty());
    assert_eq!(reader.position(), MAX_FRONTMATTER_BYTES as u64);
    let bytes = format!(
        "---\n{}\n---not-a-delimiter",
        "#".repeat(MAX_FRONTMATTER_BYTES - 8)
    );
    let (_, warnings) = read_frontmatter(&mut Cursor::new(bytes), "skill").unwrap();
    assert_eq!(warnings, [Warning::FrontmatterTooLarge]);
}

#[test]
fn agent_skills_name_and_description_boundaries() {
    // https://agentskills.io/specification: name 1-64 Unicode lowercase alphanumerics,
    // single non-edge hyphens; description 1-1024 characters, not bytes.
    for name in ["a", "abc-123", "技能", "café", &"a".repeat(64)] {
        let frontmatter = Frontmatter {
            name: Some(name.into()),
            description: Some("é".repeat(1024)),
            ..Default::default()
        };
        assert!(frontmatter.validate(name).is_empty(), "{name}");
    }
    for name in ["-abc", "abc-", "a--b", "UPPER", "a_b", "a/b", "a b", "😀"] {
        let frontmatter = Frontmatter {
            name: Some(name.into()),
            description: Some("ok".into()),
            ..Default::default()
        };
        assert!(
            frontmatter
                .validate(name)
                .contains(&Warning::NameCharacters),
            "{name}"
        );
    }
    for name in ["", &"é".repeat(65)] {
        let frontmatter = Frontmatter {
            name: Some(name.into()),
            description: Some("ok".into()),
            ..Default::default()
        };
        assert!(frontmatter.validate(name).contains(&Warning::NameLength));
    }
    for description in [
        None,
        Some("".into()),
        Some(" \n ".into()),
        Some("é".repeat(1025)),
    ] {
        let frontmatter = Frontmatter {
            name: Some("skill".into()),
            description,
            ..Default::default()
        };
        assert!(
            frontmatter
                .validate("skill")
                .contains(&Warning::DescriptionLength)
        );
    }
    assert!(
        Frontmatter::default()
            .validate("skill")
            .contains(&Warning::NameLength)
    );
}

#[test]
fn invalid_field_types_do_not_discard_other_readable_fields() {
    let bytes = b"---\nname: skill\ndescription: []\nmetadata: {author: example, bad: []}\nlicense: []\n---\n";
    let (frontmatter, warnings) = read_frontmatter(&mut Cursor::new(bytes), "skill").unwrap();
    assert_eq!(frontmatter.name.as_deref(), Some("skill"));
    assert_eq!(frontmatter.metadata["author"], "example");
    for field in ["description", "metadata", "license"] {
        assert!(warnings.contains(&Warning::InvalidField(field.into())));
    }
    for compatibility in ["".into(), "x".repeat(501)] {
        let frontmatter = Frontmatter {
            compatibility: Some(compatibility),
            ..Default::default()
        };
        assert!(
            frontmatter
                .validate("skill")
                .contains(&Warning::CompatibilityLength)
        );
    }
}

#[test]
fn supports_crlf_and_closing_delimiter_at_eof() {
    let bytes = b"---\r\nname: skill\r\ndescription: good\r\n---";
    assert!(
        read_frontmatter(&mut Cursor::new(bytes), "skill")
            .unwrap()
            .1
            .is_empty()
    );
}

#[test]
fn reads_skill_file_and_warns_if_it_is_missing() {
    let temp = tempfile::tempdir().unwrap();
    let directory = temp.path().join("valid-skill");
    std::fs::create_dir(&directory).unwrap();
    assert_eq!(read_skill(&directory).unwrap().1, [Warning::MissingSkillMd]);
    std::fs::write(directory.join("SKILL.md"), VALID).unwrap();
    assert!(read_skill(&directory).unwrap().1.is_empty());
    std::fs::write(
        directory.join("SKILL.md"),
        format!("---\n{}\n---\n", "x".repeat(MAX_FRONTMATTER_BYTES * 2)),
    )
    .unwrap();
    assert_eq!(
        read_skill(&directory).unwrap().1,
        [Warning::FrontmatterTooLarge]
    );
}

#[test]
fn io_errors_propagate_but_body_is_never_requested() {
    struct HeaderOnly(Cursor<Vec<u8>>);
    impl Read for HeaderOnly {
        fn read(&mut self, _: &mut [u8]) -> io::Result<usize> {
            panic!("read should use buffered data");
        }
    }
    impl BufRead for HeaderOnly {
        fn fill_buf(&mut self) -> io::Result<&[u8]> {
            if self.0.position() as usize == self.0.get_ref().len() {
                return Err(io::Error::other("body must not be read"));
            }
            self.0.fill_buf()
        }
        fn consume(&mut self, amount: usize) {
            self.0.consume(amount);
        }
    }
    let mut reader = HeaderOnly(Cursor::new(
        b"---\nname: skill\ndescription: good\n---\n".to_vec(),
    ));
    assert!(read_frontmatter(&mut reader, "skill").unwrap().1.is_empty());
    let mut reader = HeaderOnly(Cursor::new(b"---\nname: skill\n".to_vec()));
    assert_eq!(
        read_frontmatter(&mut reader, "skill").unwrap_err().code(),
        "E_IO"
    );
}

#[test]
#[ignore = "performance budget, run with cargo test --release -- --ignored --nocapture"]
fn ten_thousand_parses_under_200_ms() {
    if cfg!(debug_assertions) {
        panic!("run this benchmark with --release");
    }
    let start = Instant::now();
    for _ in 0..10_000 {
        let result =
            read_frontmatter(&mut Cursor::new(std::hint::black_box(VALID)), "valid-skill").unwrap();
        assert!(result.1.is_empty());
        std::hint::black_box(result);
    }
    let elapsed = start.elapsed();
    eprintln!("10,000 frontmatter parses: {elapsed:?}");
    assert!(elapsed < Duration::from_millis(200), "{elapsed:?}");
}
