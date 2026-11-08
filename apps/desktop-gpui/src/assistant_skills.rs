use std::{collections::HashSet, io::Read, path::PathBuf};

use crate::i18n::tr;

#[derive(Clone, Copy, Debug)]
pub(crate) enum QuickTask {
    Cover,
    Rewrite,
    Fillers,
    Translate,
}

impl QuickTask {
    pub(crate) const ALL: [Self; 4] = [Self::Cover, Self::Rewrite, Self::Fillers, Self::Translate];

    pub(crate) fn for_label(label: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|task| task.label() == label)
    }

    pub(crate) fn id(self) -> &'static str {
        match self {
            Self::Cover => "cover",
            Self::Rewrite => "rewrite",
            Self::Fillers => "fillers",
            Self::Translate => "translate",
        }
    }

    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Cover => tr::editor::assistant::suggest_cover(),
            Self::Rewrite => tr::editor::assistant::suggest_rewrite(),
            Self::Fillers => tr::editor::assistant::suggest_fillers(),
            Self::Translate => tr::editor::assistant::translate(),
        }
    }

    pub(crate) fn instructions(self) -> &'static str {
        match self {
            Self::Cover => {
                "Create a cover proposal grounded in the supplied captions and existing intro. Write a short specific title and an image-background brief with composition, palette and lighting, leaving uncluttered space for the editable title. Do not put lettering in the generated image. Preserve an existing duration; new covers default to one second. If the topic cannot be inferred, ask one concise question instead of inventing a subject. Return a cover brief for review; do not claim to have generated an image."
            }
            Self::Rewrite => {
                "Polish only the supplied editable subtitles. Correct obvious transcription errors, punctuation and awkward phrasing with minimal edits. Preserve the speaker's meaning, tone, names, numbers, negations and logical sequence. Use caption_words for timed words and caption_text only for sources without words. Preserve word slots and timing; omit unchanged and already-hidden words. Do not edit translations, audio, video or styles. Describe the actual scope and stage a reviewable proposal."
            }
            Self::Fillers => {
                "Remove only meaningless hesitation from the supplied subtitles. Use caption_words with hidden=true only for words explicitly marked can_hide=true. Copy the exact id, index and before text; keep after identical to before. Never remove negations, numbers, affirmative replies, meaningful repetitions, sequential then, or referring that. Omit already hidden words. If there are no eligible fillers, return no operations and explain that no safely removable fillers were found in the supplied subtitles. This edits subtitles only, not speech or media. Never claim to have listened to audio or processed captions outside this batch."
            }
            Self::Translate => {
                "Translate the supplied displayed captions into the user's requested target language. If no target language is stated in the conversation, ask which language and return no operations. Preserve meaning, names, numbers and tone. Use display caption IDs and exact source text, retain originals and enable bilingual display. Do not mix rewriting or cuts into translation. State the actual batch scope and stage changes for review."
            }
        }
    }
}

#[derive(Clone, Debug)]
pub(crate) struct Skill {
    pub path: PathBuf,
    pub name: String,
    pub body: String,
}

impl Skill {
    pub(crate) fn read(path: PathBuf) -> Option<Self> {
        let path = path.canonicalize().ok()?;
        if !path.is_file() || path.file_name()? != "SKILL.md" {
            return None;
        }
        let mut markdown = String::new();
        std::fs::File::open(&path)
            .ok()?
            .take(65_537)
            .read_to_string(&mut markdown)
            .ok()?;
        if markdown.len() > 65_536 {
            return None;
        }
        let normalized = markdown.replace("\r\n", "\n");
        let (name, body) = if let Some(rest) = normalized.strip_prefix("---\n") {
            let (frontmatter, body) = rest.split_once("\n---\n")?;
            let name = frontmatter
                .lines()
                .find_map(|line| line.strip_prefix("name:"))?
                .trim()
                .trim_matches(['\'', '"'])
                .to_owned();
            (name, body.trim().to_owned())
        } else {
            let name = path.parent()?.file_name()?.to_string_lossy().into_owned();
            (name, normalized.trim().to_owned())
        };
        if name.is_empty()
            || name.len() > 160
            || name.chars().any(char::is_control)
            || body.is_empty()
        {
            return None;
        }
        Some(Self { path, name, body })
    }
}

pub(crate) fn discover() -> Vec<Skill> {
    let Some(home) = std::env::var_os("HOME").map(PathBuf::from) else {
        return Vec::new();
    };
    discover_in(
        [
            "Nutstore Files/.agents/skills",
            ".agents/skills",
            ".claude/skills",
            ".codex/skills",
            ".cursor/skills",
            ".config/opencode/skills",
            ".pi/agent/skills",
        ]
        .into_iter()
        .map(|path| home.join(path))
        .collect(),
    )
}

fn discover_in(roots: Vec<PathBuf>) -> Vec<Skill> {
    let mut pending: Vec<_> = roots.into_iter().rev().map(|path| (path, 0)).collect();
    let mut visited = HashSet::new();
    let mut skills = Vec::new();
    let mut inspected = 0;
    while let Some((path, depth)) = pending.pop() {
        inspected += 1;
        if inspected > 4096 {
            break;
        }
        let Ok(path) = path.canonicalize() else {
            continue;
        };
        if !visited.insert(path.clone()) {
            continue;
        }
        if path.is_file() {
            if let Some(skill) = Skill::read(path) {
                skills.push(skill);
            }
        } else if path.join("SKILL.md").is_file() {
            if let Some(skill) = Skill::read(path.join("SKILL.md")) {
                skills.push(skill);
            }
        } else if depth <= 3
            && let Ok(entries) = std::fs::read_dir(path)
        {
            pending.extend(
                entries
                    .take(1024)
                    .filter_map(Result::ok)
                    .map(|entry| (entry.path(), depth + 1)),
            );
        }
    }
    skills.sort_by(|a, b| a.name.cmp(&b.name).then(a.path.cmp(&b.path)));
    skills.dedup_by(|a, b| a.path == b.path);
    skills
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn discovery_reads_instructions_deduplicates_roots_and_bounds_files() {
        let directory = tempfile::tempdir().unwrap();
        let skill = directory.path().join("caption-style");
        std::fs::create_dir(&skill).unwrap();
        std::fs::write(skill.join("SKILL.md"), "---\r\nname: 'Caption style'\r\ndescription: |\r\n  Style captions\r\n---\r\nPreserve meaning.").unwrap();
        std::fs::write(skill.join("script.sh"), "not skill instructions").unwrap();
        let found = discover_in(vec![directory.path().to_owned(), skill.clone()]);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].name, "Caption style");
        assert_eq!(found[0].body, "Preserve meaning.");
        std::fs::write(skill.join("SKILL.md"), "x".repeat(65_537)).unwrap();
        assert!(Skill::read(skill.join("SKILL.md")).is_none());
    }
}
