use std::fs;
use std::path::Path;

use crate::error::{Result, ZocliError};

struct Skill {
    name: &'static str,
    content: &'static str,
}

const SKILLS: &[Skill] = &[
    Skill {
        name: "zocli-shared",
        content: include_str!("../../skills/zocli-shared/SKILL.md"),
    },
    Skill {
        name: "zocli-mail",
        content: include_str!("../../skills/zocli-mail/SKILL.md"),
    },
    Skill {
        name: "zocli-calendar",
        content: include_str!("../../skills/zocli-calendar/SKILL.md"),
    },
    Skill {
        name: "zocli-drive",
        content: include_str!("../../skills/zocli-drive/SKILL.md"),
    },
    Skill {
        name: "zocli-daily-briefing",
        content: include_str!("../../skills/zocli-daily-briefing/SKILL.md"),
    },
    Skill {
        name: "zocli-find-and-read",
        content: include_str!("../../skills/zocli-find-and-read/SKILL.md"),
    },
    Skill {
        name: "zocli-reply-with-context",
        content: include_str!("../../skills/zocli-reply-with-context/SKILL.md"),
    },
];

pub const SKILL_COUNT: usize = 7;

pub fn skill_names() -> Vec<&'static str> {
    SKILLS.iter().map(|s| s.name).collect()
}

pub fn skill_content(name: &str) -> Option<&'static str> {
    SKILLS
        .iter()
        .find(|skill| skill.name == name)
        .map(|skill| skill.content)
}

pub fn skill_description(name: &str) -> Option<String> {
    let content = skill_content(name)?;
    parse_frontmatter_description(content)
}

pub fn skill_prompt_name(name: &str) -> Option<&'static str> {
    match name {
        "zocli-shared" => Some("shared"),
        "zocli-mail" => Some("mail"),
        "zocli-calendar" => Some("calendar"),
        "zocli-drive" => Some("drive"),
        "zocli-daily-briefing" => Some("daily-briefing"),
        "zocli-find-and-read" => Some("find-and-read"),
        "zocli-reply-with-context" => Some("reply-with-context"),
        _ => None,
    }
}

pub fn prompt_skill_name(prompt_name: &str) -> Option<&'static str> {
    SKILLS.iter().find_map(|skill| {
        (skill_prompt_name(skill.name) == Some(prompt_name)).then_some(skill.name)
    })
}

/// Write all embedded skills to `target_dir/<skill-name>/SKILL.md`.
/// Returns the number of skills written.
pub fn install_skills(target_dir: &Path) -> Result<usize> {
    for skill in SKILLS {
        let skill_dir = target_dir.join(skill.name);
        fs::create_dir_all(&skill_dir).map_err(|err| {
            ZocliError::Io(format!(
                "failed to create skill directory {}: {err}",
                skill_dir.display()
            ))
        })?;
        let skill_path = skill_dir.join("SKILL.md");
        fs::write(&skill_path, skill.content).map_err(|err| {
            ZocliError::Io(format!(
                "failed to write skill {}: {err}",
                skill_path.display()
            ))
        })?;
    }
    Ok(SKILLS.len())
}

fn parse_frontmatter_description(content: &str) -> Option<String> {
    let frontmatter = content
        .strip_prefix("---\n")?
        .split_once("\n---\n")
        .map(|(frontmatter, _)| frontmatter)?;
    let raw = frontmatter
        .lines()
        .find_map(|line| line.strip_prefix("description:"))?
        .trim();
    Some(raw.trim_matches('"').to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn skill_count_matches_constant() {
        assert_eq!(SKILLS.len(), SKILL_COUNT);
    }

    #[test]
    fn skill_names_returns_all_names() {
        let names = skill_names();
        assert_eq!(names.len(), SKILL_COUNT);
        assert!(names.contains(&"zocli-shared"));
        assert!(names.contains(&"zocli-mail"));
        assert!(names.contains(&"zocli-calendar"));
        assert!(names.contains(&"zocli-drive"));
        assert!(names.contains(&"zocli-daily-briefing"));
        assert!(names.contains(&"zocli-find-and-read"));
        assert!(names.contains(&"zocli-reply-with-context"));
    }

    #[test]
    fn skill_names_are_valid_per_spec() {
        for skill in SKILLS {
            assert!(
                skill.name.len() <= 64,
                "{}: name exceeds 64 chars",
                skill.name
            );
            assert!(
                skill
                    .name
                    .chars()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-'),
                "{}: name contains invalid characters",
                skill.name
            );
            assert!(
                !skill.name.starts_with('-') && !skill.name.ends_with('-'),
                "{}: name starts or ends with hyphen",
                skill.name
            );
            assert!(
                !skill.name.contains("--"),
                "{}: name contains consecutive hyphens",
                skill.name
            );
        }
    }

    #[test]
    fn skill_content_has_valid_frontmatter() {
        for skill in SKILLS {
            assert!(
                skill.content.starts_with("---\n"),
                "{}: missing frontmatter start",
                skill.name
            );
            let after_first = &skill.content[4..];
            assert!(
                after_first.contains("\n---\n"),
                "{}: missing frontmatter end",
                skill.name
            );

            assert!(
                skill.content.contains(&format!("name: {}", skill.name)),
                "{}: frontmatter name does not match directory name",
                skill.name
            );
            assert!(
                skill.content.contains("description:"),
                "{}: missing description in frontmatter",
                skill.name
            );
        }
    }

    #[test]
    fn skill_content_within_size_limits() {
        for skill in SKILLS {
            let line_count = skill.content.lines().count();
            assert!(
                line_count <= 500,
                "{}: {} lines exceeds 500 line limit",
                skill.name,
                line_count
            );
        }
    }

    #[test]
    fn skill_description_within_spec_limit() {
        for skill in SKILLS {
            let fm_end = skill.content[4..].find("\n---\n").unwrap() + 4;
            let frontmatter = &skill.content[4..fm_end];
            let desc_line = frontmatter
                .lines()
                .find(|line| line.starts_with("description:"))
                .unwrap_or_else(|| panic!("{}: no description line", skill.name));
            let desc = desc_line.trim_start_matches("description:").trim();
            let desc = desc.trim_matches('"');
            assert!(
                desc.len() <= 1024,
                "{}: description is {} chars, exceeds 1024",
                skill.name,
                desc.len()
            );
            assert!(!desc.is_empty(), "{}: description is empty", skill.name);
        }
    }

    #[test]
    fn install_skills_writes_all_files() {
        let temp = tempdir().expect("tempdir");
        let count = install_skills(temp.path()).expect("install");
        assert_eq!(count, SKILL_COUNT);

        for skill in SKILLS {
            let path = temp.path().join(skill.name).join("SKILL.md");
            assert!(path.exists(), "{}/SKILL.md not found", skill.name);
            let content = fs::read_to_string(&path).expect("read");
            assert_eq!(content, skill.content);
        }
    }

    #[test]
    fn install_skills_is_idempotent() {
        let temp = tempdir().expect("tempdir");
        install_skills(temp.path()).expect("first install");
        let count = install_skills(temp.path()).expect("second install");
        assert_eq!(count, SKILL_COUNT);

        for skill in SKILLS {
            let path = temp.path().join(skill.name).join("SKILL.md");
            let content = fs::read_to_string(&path).expect("read");
            assert_eq!(content, skill.content);
        }
    }

    #[test]
    fn skill_content_and_description_are_available() {
        let content = skill_content("zocli-mail").expect("skill content");
        assert!(content.contains("# zocli mail"));

        let description = skill_description("zocli-mail").expect("skill description");
        assert!(description.contains("Zoho Mail"));
    }

    #[test]
    fn prompt_and_skill_name_mapping_is_bidirectional() {
        assert_eq!(prompt_skill_name("mail"), Some("zocli-mail"));
        assert_eq!(
            prompt_skill_name("daily-briefing"),
            Some("zocli-daily-briefing")
        );
        assert_eq!(skill_prompt_name("zocli-shared"), Some("shared"));
        assert_eq!(
            skill_prompt_name("zocli-reply-with-context"),
            Some("reply-with-context")
        );
        assert_eq!(prompt_skill_name("drive"), Some("zocli-drive"));
        assert_eq!(skill_prompt_name("zocli-drive"), Some("drive"));
    }
}
