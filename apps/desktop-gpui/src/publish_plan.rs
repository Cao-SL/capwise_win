//! The publish plan: everything a finished recording needs before it goes to
//! Bilibili, YouTube or Douyin, worked out from what was said in it.
//!
//! One plan per project, saved as `publish-plan.json` next to the project.
//! It is the shared input for the automatic intro and outro cards, the
//! background music pick, the cover, and the per-platform export bundle, so
//! each of those reads the same title and mood instead of asking the user.
//!
//! With a language model configured the plan comes from [`generate`]; without
//! one [`heuristic_plan`] still fills every field from the transcript, so the
//! offline app records and exports without a single prompt.

use std::path::{Path, PathBuf};

use cap_project::ProjectConfiguration;
use serde::{Deserialize, Serialize};

const FILE: &str = "publish-plan.json";
pub(crate) const VERSION: u32 = 1;

/// Field limits, in characters. They follow the platforms' own limits where
/// those are tighter (Bilibili titles stop at 80, Douyin captions at 1000),
/// and keep the on-screen text short enough to read at a glance.
pub(crate) const TITLE_MAX: usize = 80;
pub(crate) const HEADLINE_MAX: usize = 16;
pub(crate) const SUBLINE_MAX: usize = 28;
pub(crate) const DESCRIPTION_MAX: usize = 1000;
pub(crate) const TAG_MAX: usize = 20;
pub(crate) const TAGS_MAX: usize = 10;
pub(crate) const CHAPTERS_MAX: usize = 12;
const TRANSCRIPT_BUDGET: usize = 12_000;

/// The feel of the background music, chosen from the transcript. The music
/// library maps each bundled track to one of these.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum Mood {
    /// Tutorials, walkthroughs, coding: steady and unobtrusive.
    #[default]
    Focus,
    /// Reflective talks, stories, slow explanations.
    Calm,
    /// Product launches, good news, energetic demos.
    Upbeat,
    /// Big reveals, reviews, trailers.
    Cinematic,
    /// Casual chats, vlogs, relaxed screen tours.
    Lofi,
    /// Personal, friendly, thank-you or welcome videos.
    Warm,
}

impl Mood {
    pub(crate) const ALL: [Self; 6] = [
        Self::Focus,
        Self::Calm,
        Self::Upbeat,
        Self::Cinematic,
        Self::Lofi,
        Self::Warm,
    ];

    pub(crate) fn slug(self) -> &'static str {
        match self {
            Self::Focus => "focus",
            Self::Calm => "calm",
            Self::Upbeat => "upbeat",
            Self::Cinematic => "cinematic",
            Self::Lofi => "lofi",
            Self::Warm => "warm",
        }
    }

    fn from_slug(slug: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|mood| mood.slug().eq_ignore_ascii_case(slug.trim()))
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub(crate) struct Chapter {
    /// Seconds on the edited timeline.
    pub start: f64,
    pub title: String,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub(crate) struct PlatformCopy {
    pub title: String,
    pub description: String,
    pub tags: Vec<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub(crate) struct Platforms {
    pub bilibili: PlatformCopy,
    pub youtube: PlatformCopy,
    /// Douyin has no separate title: `title` is the caption's first line and
    /// `description` the whole caption, hashtags included.
    pub douyin: PlatformCopy,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum Source {
    /// Filled from the transcript by local rules.
    #[default]
    Heuristic,
    /// Written by the configured language model.
    Model,
    /// Edited by the user; never overwritten automatically.
    Edited,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub(crate) struct PublishPlan {
    pub version: u32,
    pub source: Source,
    /// The transcript fingerprint the plan was made from; a different one
    /// means the captions changed since.
    pub fingerprint: u64,
    /// BCP-47-ish language of the transcript, e.g. `zh` or `en`.
    pub language: String,
    /// The video's title, shared by the intro card and the uploads.
    pub title: String,
    /// The cover's big words: at most [`HEADLINE_MAX`] characters.
    pub cover_headline: String,
    /// The cover's smaller second line.
    pub cover_subline: String,
    /// The intro card: the title, then one line on what the video covers.
    pub intro_title: String,
    pub intro_subtitle: String,
    /// The outro card's single line, e.g. a thank-you or a follow prompt.
    pub outro_line: String,
    pub chapters: Vec<Chapter>,
    pub mood: Mood,
    pub platforms: Platforms,
}

impl Default for PublishPlan {
    fn default() -> Self {
        Self {
            version: VERSION,
            source: Source::Heuristic,
            fingerprint: 0,
            language: String::new(),
            title: String::new(),
            cover_headline: String::new(),
            cover_subline: String::new(),
            intro_title: String::new(),
            intro_subtitle: String::new(),
            outro_line: String::new(),
            chapters: Vec::new(),
            mood: Mood::default(),
            platforms: Platforms::default(),
        }
    }
}

/// One spoken line on the edited timeline.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Line {
    pub start: f64,
    pub end: f64,
    pub text: String,
}

/// The transcript as the viewer hears it: the timeline's caption track when
/// there is one (already in edited time), else the raw caption segments.
pub(crate) fn transcript(project: &ProjectConfiguration) -> Vec<Line> {
    let from_timeline: Vec<Line> = project
        .timeline
        .as_ref()
        .map(|timeline| {
            timeline
                .caption_segments
                .iter()
                .map(|segment| Line {
                    start: segment.start,
                    end: segment.end,
                    text: segment.text.trim().to_string(),
                })
                .filter(|line| !line.text.is_empty())
                .collect()
        })
        .unwrap_or_default();
    if !from_timeline.is_empty() {
        return from_timeline;
    }
    project
        .captions
        .as_ref()
        .map(|captions| {
            captions
                .segments
                .iter()
                .map(|segment| Line {
                    start: f64::from(segment.start),
                    end: f64::from(segment.end),
                    text: segment.text.trim().to_string(),
                })
                .filter(|line| !line.text.is_empty())
                .collect()
        })
        .unwrap_or_default()
}

pub(crate) fn fingerprint(lines: &[Line]) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hash = std::collections::hash_map::DefaultHasher::new();
    for line in lines {
        line.text.hash(&mut hash);
        line.start.to_bits().hash(&mut hash);
    }
    hash.finish()
}

fn is_cjk(text: &str) -> bool {
    let cjk = text
        .chars()
        .filter(|c| cap_project::is_unspaced_script(*c))
        .count();
    let letters = text.chars().filter(|c| c.is_alphabetic()).count();
    letters > 0 && cjk * 3 >= letters
}

fn truncate_chars(text: &str, max: usize) -> String {
    let text = text.trim();
    if text.chars().count() <= max {
        return text.to_string();
    }
    let cut: String = text.chars().take(max).collect();
    // A spaced language is cut at the last whole word, not mid-word.
    let next = text.chars().nth(max);
    if !is_cjk(&cut)
        && next.is_some_and(|c| !c.is_whitespace())
        && let Some(space) = cut.rfind(char::is_whitespace)
    {
        return cut[..space].trim_end().to_string();
    }
    cut.trim_end().to_string()
}

fn is_clause_break(c: char) -> bool {
    matches!(c, '。' | '！' | '？' | '，' | '；' | '.' | '!' | '?' | ',' | ';')
}

/// Cuts a sentence at its first clause break, so a long opening line still
/// reads as a title.
fn first_clause(text: &str) -> &str {
    let text = text.trim();
    let end = text.find(is_clause_break).unwrap_or(text.len());
    text[..end].trim()
}

/// Whether a clause says enough to stand as a title: four characters of
/// Chinese, or three words of a spaced language.
fn meaningful(clause: &str) -> bool {
    if is_cjk(clause) {
        clause.chars().filter(|c| c.is_alphanumeric()).count() >= 4
    } else {
        clause.split_whitespace().count() >= 3
    }
}

/// Spoken fillers a title should not start with.
const OPENERS: &[&str] = &[
    "大家好", "哈喽", "嗨", "那么", "今天", "我们", "嗯", "呃", "啊", "好的", "OK", "ok",
    "Hi", "Hello", "Hey", "So", "Okay", "Today",
];

fn strip_opener(text: &str) -> &str {
    let mut text = text.trim();
    loop {
        let before = text;
        for opener in OPENERS {
            if let Some(rest) = text.strip_prefix(opener) {
                text = rest.trim_start_matches([',', '，', ' ', '、', '!', '！']).trim();
            }
        }
        if text == before {
            return text;
        }
    }
}

/// Words that suggest a mood, checked against the whole transcript.
const MOOD_HINTS: &[(Mood, &[&str])] = &[
    (
        Mood::Upbeat,
        &["发布", "上线", "新功能", "惊喜", "太棒", "launch", "release", "new feature", "excited"],
    ),
    (
        Mood::Cinematic,
        &["评测", "揭秘", "震撼", "对比", "review", "reveal", "versus", "trailer"],
    ),
    (
        Mood::Warm,
        &["感谢", "谢谢", "分享", "故事", "生活", "thank", "story", "personal"],
    ),
    (
        Mood::Calm,
        &["思考", "反思", "理解", "原理", "为什么", "think", "reflect", "why"],
    ),
    (
        Mood::Lofi,
        &["聊聊", "随便", "日常", "vlog", "chat", "casual"],
    ),
];

fn guess_mood(text: &str) -> Mood {
    let lower = text.to_lowercase();
    MOOD_HINTS
        .iter()
        .map(|(mood, words)| {
            let hits: usize = words.iter().map(|word| lower.matches(word).count()).sum();
            (hits, *mood)
        })
        .filter(|(hits, _)| *hits > 0)
        .max_by_key(|(hits, _)| *hits)
        .map_or(Mood::Focus, |(_, mood)| mood)
}

/// Evenly spaced chapters on sentence starts, one per ~90 seconds; none for a
/// short video, where chapters only add noise.
fn guess_chapters(lines: &[Line]) -> Vec<Chapter> {
    let Some(last) = lines.last() else {
        return Vec::new();
    };
    let duration = last.end;
    if duration < 180. || lines.len() < 6 {
        return Vec::new();
    }
    let count = ((duration / 90.).round() as usize).clamp(3, CHAPTERS_MAX);
    let step = duration / count as f64;
    let mut chapters = vec![Chapter {
        start: 0.,
        title: truncate_chars(first_clause(strip_opener(&lines[0].text)), 20),
    }];
    for index in 1..count {
        let target = step * index as f64;
        if let Some(line) = lines.iter().find(|line| line.start >= target) {
            let title = truncate_chars(first_clause(strip_opener(&line.text)), 20);
            if !title.is_empty() && chapters.last().is_none_or(|last| last.start < line.start) {
                chapters.push(Chapter {
                    start: line.start,
                    title,
                });
            }
        }
    }
    chapters.retain(|chapter| !chapter.title.is_empty());
    chapters
}

/// A plan from local rules only: the first meaningful sentence becomes the
/// title, the mood comes from keywords, and each platform gets a plain copy.
pub(crate) fn heuristic_plan(lines: &[Line]) -> PublishPlan {
    let all: String = lines
        .iter()
        .map(|line| line.text.as_str())
        .collect::<Vec<_>>()
        .join(" ");
    let chinese = is_cjk(&all);
    let opening = lines
        .iter()
        .flat_map(|line| line.text.split(is_clause_break))
        .map(strip_opener)
        .find(|clause| meaningful(clause))
        .unwrap_or_default();
    let fallback_title = if chinese { "录屏分享" } else { "Screen recording" };
    let title = if opening.is_empty() {
        fallback_title.to_string()
    } else {
        truncate_chars(opening, if chinese { 30 } else { 60 })
    };
    let headline = truncate_chars(&title, HEADLINE_MAX);
    let summary = truncate_chars(
        &lines
            .iter()
            .take(6)
            .map(|line| line.text.as_str())
            .collect::<Vec<_>>()
            .join(if chinese { "" } else { " " }),
        160,
    );
    let chapters = guess_chapters(lines);
    let chapter_text = chapter_lines(&chapters);
    let description = if chapter_text.is_empty() {
        summary.clone()
    } else {
        format!("{summary}\n\n{chapter_text}")
    };
    let outro = if chinese {
        "感谢观看，下期见"
    } else {
        "Thanks for watching"
    };
    PublishPlan {
        version: VERSION,
        source: Source::Heuristic,
        fingerprint: fingerprint(lines),
        language: if chinese { "zh" } else { "en" }.into(),
        title: title.clone(),
        cover_headline: headline,
        cover_subline: String::new(),
        intro_title: title.clone(),
        intro_subtitle: String::new(),
        outro_line: outro.into(),
        chapters,
        mood: guess_mood(&all),
        platforms: Platforms {
            bilibili: PlatformCopy {
                title: title.clone(),
                description: description.clone(),
                tags: Vec::new(),
            },
            youtube: PlatformCopy {
                title: title.clone(),
                description,
                tags: Vec::new(),
            },
            douyin: PlatformCopy {
                title: title.clone(),
                description: title,
                tags: Vec::new(),
            },
        },
    }
}

/// `00:00 Title` lines, the format Bilibili and YouTube both turn into
/// chapter markers when the first one starts at zero.
pub(crate) fn chapter_lines(chapters: &[Chapter]) -> String {
    chapters
        .iter()
        .map(|chapter| {
            let seconds = chapter.start.max(0.).round() as u64;
            let stamp = if seconds >= 3600 {
                format!(
                    "{}:{:02}:{:02}",
                    seconds / 3600,
                    seconds / 60 % 60,
                    seconds % 60
                )
            } else {
                format!("{:02}:{:02}", seconds / 60, seconds % 60)
            };
            format!("{stamp} {}", chapter.title)
        })
        .collect::<Vec<_>>()
        .join("\n")
}

const PROMPT: &str = r#"You prepare a screen recording for upload. You get its transcript as JSON lines [start_seconds, text]. Reply with ONE JSON object and nothing else:
{
 "language": "zh" | "en" | ...,            // the transcript's language; write every field in it
 "title": string,                          // <= 40 chars for zh, <= 70 for en; specific, no clickbait, no emoji
 "coverHeadline": string,                  // <= 12 chars for zh, <= 5 words for en; the hook, readable on a phone
 "coverSubline": string,                   // <= 24 chars; what the viewer gets
 "introTitle": string,                     // usually the title, may be shorter
 "introSubtitle": string,                  // one line on what the video covers
 "outroLine": string,                      // short thank-you or follow prompt
 "chapters": [{"start": number, "title": string}], // only for videos over 3 minutes; first starts at 0; 3-10 entries; titles <= 16 chars
 "mood": "focus"|"calm"|"upbeat"|"cinematic"|"lofi"|"warm", // background music feel
 "bilibili": {"title": string, "description": string, "tags": [string]},  // tags: 5-10, no '#'
 "youtube": {"title": string, "description": string, "tags": [string]},   // description ends with the chapter list if any
 "douyin": {"title": string, "description": string, "tags": [string]}     // description: 1-2 short lines plus #hashtags
}
Only use facts from the transcript. Keep names, product terms and numbers exactly as spoken."#;

fn transcript_message(lines: &[Line]) -> String {
    let mut budget = TRANSCRIPT_BUDGET;
    let mut rows = Vec::new();
    for line in lines {
        let cost = line.text.chars().count() + 12;
        if cost > budget {
            break;
        }
        budget -= cost;
        rows.push(serde_json::json!([(line.start * 10.).round() / 10., line.text]));
    }
    serde_json::Value::Array(rows).to_string()
}

fn json_object(reply: &str) -> Option<serde_json::Map<String, serde_json::Value>> {
    let start = reply.find('{')?;
    let end = reply.rfind('}')?;
    serde_json::from_str(reply.get(start..=end)?).ok()
}

fn text(value: Option<&serde_json::Value>, max: usize) -> Option<String> {
    let text = truncate_chars(value?.as_str()?, max);
    (!text.is_empty()).then_some(text)
}

fn tags(value: Option<&serde_json::Value>) -> Vec<String> {
    let mut tags: Vec<String> = Vec::new();
    for tag in value.and_then(|value| value.as_array()).into_iter().flatten() {
        let Some(tag) = tag.as_str() else { continue };
        let tag = truncate_chars(tag.trim().trim_start_matches('#'), TAG_MAX);
        if !tag.is_empty() && !tags.iter().any(|seen| seen.eq_ignore_ascii_case(&tag)) {
            tags.push(tag);
        }
        if tags.len() == TAGS_MAX {
            break;
        }
    }
    tags
}

fn platform(value: Option<&serde_json::Value>, fallback: &PlatformCopy) -> PlatformCopy {
    let object = value.and_then(|value| value.as_object());
    PlatformCopy {
        title: text(object.and_then(|o| o.get("title")), TITLE_MAX)
            .unwrap_or_else(|| fallback.title.clone()),
        description: text(object.and_then(|o| o.get("description")), DESCRIPTION_MAX)
            .unwrap_or_else(|| fallback.description.clone()),
        tags: tags(object.and_then(|o| o.get("tags"))),
    }
}

/// Reads the model's reply over the heuristic plan, so any field the model
/// left out or got wrong keeps a usable value. `duration` bounds chapters.
pub(crate) fn parse_reply(reply: &str, base: &PublishPlan, duration: f64) -> Option<PublishPlan> {
    let object = json_object(reply)?;
    let get = |key: &str| object.get(key);
    let mut chapters: Vec<Chapter> = get("chapters")
        .and_then(|value| value.as_array())
        .into_iter()
        .flatten()
        .filter_map(|chapter| {
            let start = chapter.get("start")?.as_f64()?;
            let title = text(chapter.get("title"), 24)?;
            (start.is_finite() && (0. ..duration.max(0.)).contains(&start))
                .then_some(Chapter { start, title })
        })
        .collect();
    chapters.sort_by(|a, b| a.start.total_cmp(&b.start));
    chapters.dedup_by(|a, b| (a.start - b.start).abs() < 10.);
    chapters.truncate(CHAPTERS_MAX);
    // Platforms only show chapters that start at zero.
    if chapters.first().is_some_and(|first| first.start > 0.) {
        chapters[0].start = 0.;
    }
    if chapters.len() < 3 {
        chapters.clear();
    }
    let title = text(get("title"), TITLE_MAX).unwrap_or_else(|| base.title.clone());
    Some(PublishPlan {
        version: VERSION,
        source: Source::Model,
        fingerprint: base.fingerprint,
        language: text(get("language"), 12).unwrap_or_else(|| base.language.clone()),
        cover_headline: text(get("coverHeadline"), HEADLINE_MAX)
            .unwrap_or_else(|| truncate_chars(&title, HEADLINE_MAX)),
        cover_subline: text(get("coverSubline"), SUBLINE_MAX).unwrap_or_default(),
        intro_title: text(get("introTitle"), TITLE_MAX).unwrap_or_else(|| title.clone()),
        intro_subtitle: text(get("introSubtitle"), 60).unwrap_or_default(),
        outro_line: text(get("outroLine"), 40).unwrap_or_else(|| base.outro_line.clone()),
        mood: get("mood")
            .and_then(|value| value.as_str())
            .and_then(Mood::from_slug)
            .unwrap_or(base.mood),
        platforms: Platforms {
            bilibili: platform(get("bilibili"), &base.platforms.bilibili),
            youtube: platform(get("youtube"), &base.platforms.youtube),
            douyin: platform(get("douyin"), &base.platforms.douyin),
        },
        chapters,
        title,
    })
}

/// Asks the default language model for a plan, falling back to the
/// heuristic plan when none is configured or the reply is unusable. Never
/// fails: the export path must not stop on a network error.
pub(crate) async fn generate(lines: Vec<Line>) -> PublishPlan {
    let base = heuristic_plan(&lines);
    if lines.is_empty() {
        return base;
    }
    let Some(provider) = crate::llm::default_content_model() else {
        return base;
    };
    let duration = lines.last().map_or(0., |line| line.end);
    match provider.chat(PROMPT, &transcript_message(&lines)).await {
        Ok(reply) => parse_reply(&reply, &base, duration).unwrap_or(base),
        Err(error) => {
            tracing::warn!(%error, "publish plan: model call failed; using the transcript rules");
            base
        }
    }
}

pub(crate) fn path(project: &Path) -> PathBuf {
    project.join(FILE)
}

pub(crate) fn load(project: &Path) -> Option<PublishPlan> {
    let text = std::fs::read_to_string(path(project)).ok()?;
    serde_json::from_str(&text).ok()
}

pub(crate) fn save(project: &Path, plan: &PublishPlan) -> Result<(), String> {
    let json = serde_json::to_vec_pretty(plan).map_err(|error| error.to_string())?;
    let target = path(project);
    let staged = target.with_extension("json.tmp");
    std::fs::write(&staged, json).map_err(|error| error.to_string())?;
    std::fs::rename(&staged, &target).map_err(|error| error.to_string())
}

/// Whether the saved plan should be regenerated for this transcript: missing,
/// older format, or made from different captions. A user-edited plan is kept.
pub(crate) fn needs_refresh(saved: Option<&PublishPlan>, lines: &[Line]) -> bool {
    match saved {
        None => true,
        Some(plan) if plan.source == Source::Edited => false,
        Some(plan) => plan.version != VERSION || plan.fingerprint != fingerprint(lines),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line(start: f64, text: &str) -> Line {
        Line {
            start,
            end: start + 4.,
            text: text.into(),
        }
    }

    #[test]
    fn heuristic_title_skips_greetings_and_cuts_at_the_first_clause() {
        let plan = heuristic_plan(&[
            line(0., "大家好，今天给大家演示灵录的自动字幕功能，非常好用。"),
            line(4., "首先打开编辑器。"),
        ]);
        assert_eq!(plan.title, "给大家演示灵录的自动字幕功能");
        assert_eq!(plan.language, "zh");
        assert_eq!(plan.cover_headline, plan.title);
        assert_eq!(plan.outro_line, "感谢观看，下期见");
        assert!(plan.chapters.is_empty());
    }

    #[test]
    fn heuristic_plan_handles_english_and_an_empty_transcript() {
        let plan = heuristic_plan(&[line(0., "Hey everyone. So this is the new release of Capwise.")]);
        assert_eq!(plan.language, "en");
        assert_eq!(plan.title, "this is the new release of Capwise");
        assert_eq!(plan.mood, Mood::Upbeat);
        let empty = heuristic_plan(&[]);
        assert_eq!(empty.title, "Screen recording");
        assert_eq!(empty.mood, Mood::Focus);
    }

    #[test]
    fn long_videos_get_chapters_from_zero() {
        let lines: Vec<Line> = (0..80)
            .map(|index| line(index as f64 * 5., &format!("第{index}段讲解内容，继续。")))
            .collect();
        let plan = heuristic_plan(&lines);
        assert!(plan.chapters.len() >= 3);
        assert_eq!(plan.chapters[0].start, 0.);
        assert!(plan.chapters.windows(2).all(|pair| pair[0].start < pair[1].start));
        assert!(plan.platforms.youtube.description.contains("00:00 "));
    }

    #[test]
    fn model_reply_is_bounded_and_fills_gaps_from_the_base() {
        let base = heuristic_plan(&[line(0., "演示一下新的导出功能。")]);
        let reply = r##"Sure! {"title":"三分钟学会灵录一键导出","coverHeadline":"录完就能发，再也不用剪辑软件了",
            "mood":"UPBEAT","chapters":[{"start":5,"title":"开始"},{"start":60,"title":"导出"},{"start":130,"title":"上传"},{"start":9999,"title":"越界"}],
            "bilibili":{"title":"灵录一键导出","tags":["#灵录","录屏","录屏","效率工具"]},
            "douyin":{"description":"录完就能发 #灵录"}}"##;
        let plan = parse_reply(reply, &base, 200.).unwrap();
        assert_eq!(plan.source, Source::Model);
        assert_eq!(plan.title, "三分钟学会灵录一键导出");
        assert_eq!(plan.cover_headline, "录完就能发，再也不用剪辑软件了");
        assert_eq!(plan.mood, Mood::Upbeat);
        assert_eq!(plan.chapters.len(), 3);
        assert_eq!(plan.chapters[0].start, 0.);
        assert_eq!(plan.platforms.bilibili.tags, vec!["灵录", "录屏", "效率工具"]);
        assert_eq!(plan.platforms.youtube.title, base.platforms.youtube.title);
        assert_eq!(plan.platforms.douyin.description, "录完就能发 #灵录");
        assert!(parse_reply("no json here", &base, 200.).is_none());
    }

    #[test]
    fn truncation_keeps_whole_words_in_spaced_languages() {
        assert_eq!(truncate_chars("record once and export everywhere", 20), "record once and");
        assert_eq!(truncate_chars("录完就能发再也不用剪辑软件了", 5), "录完就能发");
        assert_eq!(truncate_chars("short", 20), "short");
    }

    #[test]
    fn chapter_lines_use_platform_timestamps() {
        let text = chapter_lines(&[
            Chapter { start: 0., title: "开场".into() },
            Chapter { start: 95.4, title: "演示".into() },
            Chapter { start: 3725., title: "总结".into() },
        ]);
        assert_eq!(text, "00:00 开场\n01:35 演示\n1:02:05 总结");
    }

    #[test]
    fn edited_plans_are_never_refreshed() {
        let lines = [line(0., "你好")];
        let mut plan = heuristic_plan(&lines);
        assert!(!needs_refresh(Some(&plan), &lines));
        assert!(needs_refresh(Some(&plan), &[line(0., "改了")]));
        plan.source = Source::Edited;
        assert!(!needs_refresh(Some(&plan), &[line(0., "改了")]));
        assert!(needs_refresh(None, &lines));
    }

    #[test]
    fn plans_round_trip_through_the_project_folder() {
        let dir = tempfile::tempdir().unwrap();
        let plan = heuristic_plan(&[line(0., "保存测试一下。")]);
        save(dir.path(), &plan).unwrap();
        assert_eq!(load(dir.path()), Some(plan));
    }
}
