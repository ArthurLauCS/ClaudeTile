//! The user's own Claude Code skills, listed on the tile so they can be recalled at a glance.
//! Source: <claude dir>/skills/*/SKILL.md (frontmatter `name` and `description`).
//! Left out: folders without SKILL.md, links and junctions (installed by other tools),
//! `lark-*`, and names the user switched "off" under `skillOverrides` in settings.json.

use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use crate::json::Json;

#[derive(Clone, Debug, PartialEq)]
pub struct Skill {
    pub name: String,
    /// First sentence of the description, for the one-line row.
    pub summary: String,
    /// Whole description, for the hover text.
    pub desc: String,
}

const REPARSE_POINT: u32 = 0x400;

fn is_link(p: &Path) -> bool {
    use std::os::windows::fs::MetadataExt;
    std::fs::symlink_metadata(p).map(|m| m.file_type().is_symlink() || m.file_attributes() & REPARSE_POINT != 0).unwrap_or(true)
}

fn mtime(p: &Path) -> Option<SystemTime> {
    std::fs::metadata(p).and_then(|m| m.modified()).ok()
}

/// Names switched off in settings.json. Entries tiered to "name-only" stay: they are still the user's skills.
fn switched_off(claude: &Path) -> Vec<String> {
    let text = std::fs::read_to_string(claude.join("settings.json")).unwrap_or_default();
    match Json::parse(text.trim_start_matches('\u{feff}')).as_ref().map(|j| j.get("skillOverrides")) {
        Some(Json::Obj(m)) => m.iter().filter(|(_, v)| v.str() == Some("off")).map(|(k, _)| k.clone()).collect(),
        _ => Vec::new(),
    }
}

/// Candidate skill folders: (name, SKILL.md path), sorted by name.
fn candidates(claude: &Path) -> Vec<(String, PathBuf)> {
    let off = switched_off(claude);
    let mut v: Vec<(String, PathBuf)> = std::fs::read_dir(claude.join("skills"))
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().into_owned();
            let dir = e.path();
            if name.starts_with('.') || name.starts_with("lark-") || off.contains(&name) || is_link(&dir) {
                return None;
            }
            let md = dir.join("SKILL.md");
            md.is_file().then_some((name, md))
        })
        .collect();
    v.sort();
    v
}

/// Cheap change detector for the 15 s tick: names and modification times, no file contents.
pub fn signature(claude: &Path) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    for (name, md) in candidates(claude) {
        (name, mtime(&md)).hash(&mut h);
    }
    mtime(&claude.join("settings.json")).hash(&mut h);
    h.finish()
}

pub fn load(claude: &Path) -> Vec<Skill> {
    candidates(claude)
        .into_iter()
        .filter_map(|(dir_name, md)| {
            let text = std::fs::read_to_string(md).ok()?;
            let (name, desc) = frontmatter(&text);
            let name = if name.is_empty() { dir_name } else { name };
            Some(Skill { summary: summary(&desc), name, desc })
        })
        .collect()
}

/// `name` and `description` from the leading `---` block. Descriptions may be plain, quoted,
/// or a folded / literal block (`>-`, `>`, `|`) on the indented lines below.
pub fn frontmatter(text: &str) -> (String, String) {
    let mut lines = text.trim_start_matches('\u{feff}').lines();
    if lines.next().map(str::trim) != Some("---") {
        return Default::default();
    }
    let (mut name, mut desc) = (String::new(), String::new());
    let mut in_desc = false;
    for line in lines {
        if line.trim() == "---" {
            break;
        }
        if in_desc && (line.starts_with(' ') || line.starts_with('\t')) {
            if !desc.is_empty() && !desc.ends_with(' ') {
                desc.push(' ');
            }
            desc += line.trim();
            continue;
        }
        in_desc = false;
        if let Some(v) = line.strip_prefix("name:") {
            name = unquote(v.trim());
        } else if let Some(v) = line.strip_prefix("description:") {
            let v = v.trim();
            if matches!(v.chars().next(), Some('>' | '|')) {
                in_desc = true;
            } else {
                desc = unquote(v);
            }
        }
    }
    (name, desc.trim().to_string())
}

fn unquote(v: &str) -> String {
    let b = v.as_bytes();
    if b.len() >= 2 && (b[0] == b'"' || b[0] == b'\'') && b[b.len() - 1] == b[0] {
        return v[1..v.len() - 1].replace("\\\"", "\"");
    }
    v.to_string()
}

/// First sentence: up to the first ". " / "." at the end / "。" / "；".
/// A period followed by a lowercase word ("e.g. x") or inside a path-like word does not end it.
pub fn summary(desc: &str) -> String {
    let chars: Vec<char> = desc.chars().collect();
    for (i, &c) in chars.iter().enumerate() {
        let end = match c {
            '。' | '；' => true,
            '.' => chars.get(i + 1).is_none_or(|n| n.is_whitespace()) && !chars[..i].iter().rev().take(2).any(|x| *x == '.'),
            _ => false,
        };
        if end {
            return chars[..i].iter().collect::<String>().trim().to_string();
        }
    }
    desc.trim().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn folded_block() {
        let t = "---\nname: bug-avoidance\ndescription: >-\n  Bug-avoidance rules for the repos.\n  Covers races across yields.\nmetadata: x\n---\n# body";
        let (n, d) = frontmatter(t);
        assert_eq!(n, "bug-avoidance");
        assert_eq!(d, "Bug-avoidance rules for the repos. Covers races across yields.");
        assert_eq!(summary(&d), "Bug-avoidance rules for the repos");
    }

    #[test]
    fn inline_and_quoted() {
        let (n, d) = frontmatter("---\nname: claude-config\ndescription: How to change the setup. Load before editing (\"settings.json\", \"上下文\")\n---\n");
        assert_eq!((n.as_str(), summary(&d).as_str()), ("claude-config", "How to change the setup"));
        let (_, d) = frontmatter("---\nname: a\ndescription: \"飞书审批：查询。其他\"\n---\n");
        assert_eq!(summary(&d), "飞书审批：查询");
        assert_eq!(frontmatter("no front"), Default::default());
    }

    #[test]
    fn abbreviations_do_not_split() {
        assert_eq!(summary("Edit files e.g. a b. Then"), "Edit files e.g. a b");
        assert_eq!(summary("Only one sentence"), "Only one sentence");
    }
}
