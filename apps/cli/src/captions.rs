use std::{
    fs,
    path::{Path, PathBuf},
};

use crate::project_edit::{Result, TemporaryFile, error, io_error, revision, write_checked};
use cap_project::{
    CaptionDisplayMode, CaptionSettings, CaptionTrackSegment, ProjectConfiguration, RecordingMeta,
    RecordingMetaInner, SubtitleFormat,
};
use clap::{Args, Subcommand, ValueEnum};
use serde_json::{Value, json};

use crate::{OutputFormat, resolve_format, write_json};

#[derive(Args)]
pub struct CaptionArgs {
    #[command(subcommand)]
    command: CaptionCommand,
}

#[derive(Subcommand)]
enum CaptionCommand {
    #[command(about = "Read captions in edited output time, settings, and translation coverage")]
    Get(CaptionTarget),
    #[command(
        about = "Export SRT or VTT to an explicit new file; never overwrite an existing file"
    )]
    Export(CaptionExport),
    #[command(about = "Patch only supplied caption settings; use --dry-run to preview")]
    Style(CaptionStyle),
}

#[derive(Args)]
struct CaptionTarget {
    project_path: PathBuf,
    #[arg(long, value_enum, default_value_t = OutputFormat::Text)]
    format: OutputFormat,
}

#[derive(Clone, Copy, Debug, ValueEnum)]
enum DisplayMode {
    Original,
    Translated,
    Bilingual,
}

impl From<DisplayMode> for CaptionDisplayMode {
    fn from(value: DisplayMode) -> Self {
        match value {
            DisplayMode::Original => Self::Original,
            DisplayMode::Translated => Self::Translated,
            DisplayMode::Bilingual => Self::Bilingual,
        }
    }
}

#[derive(Clone, Copy, Debug, ValueEnum)]
enum FileFormat {
    Srt,
    Vtt,
}

impl FileFormat {
    fn extension(self) -> &'static str {
        match self {
            Self::Srt => "srt",
            Self::Vtt => "vtt",
        }
    }
}

#[derive(Args)]
struct CaptionExport {
    #[command(flatten)]
    target: CaptionTarget,
    #[arg(long, value_enum, default_value = "srt")]
    subtitle_format: FileFormat,
    #[arg(
        long,
        help = "New destination path; its parent directory must already exist"
    )]
    output: PathBuf,
    #[arg(
        long,
        value_enum,
        help = "Override presentation for this export without saving settings"
    )]
    display_mode: Option<DisplayMode>,
    #[arg(
        long,
        help = "Return the subtitle content and destination without creating any file"
    )]
    dry_run: bool,
}

#[derive(Args, Default)]
struct StylePatch {
    #[arg(long, action = clap::ArgAction::Set)]
    enabled: Option<bool>,
    #[arg(long, value_enum)]
    display_mode: Option<DisplayMode>,
    #[arg(long, help = "Original caption font family")]
    font: Option<String>,
    #[arg(long, help = "Original caption font size, 12–100")]
    size: Option<u32>,
    #[arg(long, help = "Translated caption font family")]
    translation_font: Option<String>,
    #[arg(long, help = "Translated caption font size, 12–100")]
    translation_size: Option<u32>,
    #[arg(
        long,
        help = "Space between original and translated text blocks at 1080p, 0–200 px"
    )]
    translation_gap: Option<u32>,
    #[arg(
        long,
        help = "Bottom inset of the subtitle box at 1080p, 0–400 px; manual positions are unchanged"
    )]
    bottom_margin: Option<u32>,
    #[arg(long, help = "Text color, #RRGGBB")]
    color: Option<String>,
    #[arg(long, help = "Subtitle background opacity, 0–100")]
    background_opacity: Option<u32>,
    #[arg(long, action = clap::ArgAction::Set)]
    outline: Option<bool>,
    #[arg(long, help = "Outline thickness as a percentage of font size, 0–20")]
    outline_width: Option<f32>,
    #[arg(long, help = "Outline color, #RRGGBB")]
    outline_color: Option<String>,
    #[arg(long, action = clap::ArgAction::Set)]
    shadow: Option<bool>,
    #[arg(long, help = "Shadow strength, 0–100")]
    shadow_opacity: Option<u32>,
    #[arg(long, help = "Shadow spread as a percentage of font size, 0–30")]
    shadow_size: Option<f32>,
    #[arg(long, help = "Shadow color, #RRGGBB")]
    shadow_color: Option<String>,
}

#[derive(Args)]
struct CaptionStyle {
    #[command(flatten)]
    target: CaptionTarget,
    #[command(flatten)]
    patch: StylePatch,
    #[arg(
        long,
        help = "Return before/after settings without writing the project"
    )]
    dry_run: bool,
    #[arg(
        long,
        help = "Require the revision returned by captions get or a prior dry-run"
    )]
    expected_revision: Option<String>,
}

struct Snapshot {
    path: PathBuf,
    bytes: Option<Vec<u8>>,
    raw: Value,
    project: ProjectConfiguration,
    meta: RecordingMeta,
}

impl Snapshot {
    fn load(path: &Path) -> Result<Self> {
        let meta = RecordingMeta::load_for_project(path)
            .map_err(|cause| error("INVALID_PROJECT", cause.to_string()))?;
        let bytes = match fs::read(path.join("project-config.json")) {
            Ok(bytes) => Some(bytes),
            Err(cause) if cause.kind() == std::io::ErrorKind::NotFound => None,
            Err(cause) => return Err(io_error(cause)),
        };
        let raw = bytes
            .as_ref()
            .map_or_else(|| Ok(json!({})), |bytes| serde_json::from_slice(bytes))
            .map_err(|cause| error("INVALID_PROJECT", cause.to_string()))?;
        let project: ProjectConfiguration = serde_json::from_value(raw.clone())
            .map_err(|cause| error("INVALID_PROJECT", cause.to_string()))?;
        project
            .validate()
            .map_err(|cause| error("INVALID_PROJECT", cause.to_string()))?;
        Ok(Self {
            path: path.to_path_buf(),
            bytes,
            raw,
            project,
            meta,
        })
    }

    fn revision(&self) -> String {
        revision(self.bytes.as_deref().unwrap_or_default())
    }

    fn settings(&self) -> CaptionSettings {
        self.project
            .captions
            .as_ref()
            .map(|captions| captions.settings.clone())
            .unwrap_or_default()
    }

    fn segments(&self) -> Result<Vec<CaptionTrackSegment>> {
        let mut project = self.project.clone();
        if project
            .captions
            .as_ref()
            .is_some_and(|captions| !captions.segments.is_empty())
        {
            let RecordingMetaInner::Studio(studio) = &self.meta.inner else {
                return Err(error(
                    "INVALID_PROJECT",
                    "Caption projection requires a studio project",
                ));
            };
            let recordings = cap_rendering::ProjectRecordingsMeta::new(&self.path, studio)
                .map_err(|cause| error("MEDIA_UNAVAILABLE", cause))?;
            cap_export::synthesize_default_timeline(&mut project, &recordings);
            let durations: Vec<_> = recordings
                .segments
                .iter()
                .map(|segment| segment.display.duration)
                .collect();
            cap_project::clamp_timeline_to_sources(&mut project, &durations);
            cap_project::synchronize_captions(&mut project, &durations);
        }
        Ok(project
            .timeline
            .map(|timeline| timeline.caption_segments)
            .unwrap_or_default())
    }
}

fn coverage(segments: &[CaptionTrackSegment], settings: &CaptionSettings) -> Value {
    let mut original = settings.clone();
    original.display_mode = CaptionDisplayMode::Original;
    original.uppercase = false;
    let mut total = 0;
    let mut translated = 0;
    for segment in segments {
        let source = cap_project::caption_presentation_text(segment, &original);
        if source.is_empty() {
            continue;
        }
        total += 1;
        if settings.translation_for(&segment.id, &source).is_some() {
            translated += 1;
        }
    }
    json!({"total": total, "translated": translated, "fallbackToOriginal": total - translated})
}

fn validate_mode(segments: &[CaptionTrackSegment], settings: &CaptionSettings) -> Result<Value> {
    let coverage = coverage(segments, settings);
    if settings.display_mode != CaptionDisplayMode::Original && coverage["translated"] == 0 {
        return Err(error(
            "NO_TRANSLATIONS",
            "No current translation matches these captions; use original mode or translate first",
        ));
    }
    Ok(coverage)
}

fn get(target: &CaptionTarget) -> Result<Value> {
    let snapshot = Snapshot::load(&target.project_path)?;
    let settings = snapshot.settings();
    let segments = snapshot.segments()?;
    let captions: Vec<_> = segments
        .iter()
        .map(|segment| {
            json!({
                "id": segment.id, "start": segment.start, "end": segment.end,
                "sourceText": segment.text,
                "text": cap_project::caption_presentation_text(segment, &settings),
                "words": segment.words,
            })
        })
        .collect();
    Ok(json!({
        "schemaVersion": 1, "operation": "captions.get", "projectPath": target.project_path,
        "revision": snapshot.revision(), "timeBasis": "edited-output-seconds",
        "coverage": coverage(&segments, &settings), "settings": settings, "captions": captions,
    }))
}

fn validate_destination(path: &Path, format: FileFormat) -> Result<()> {
    if path.extension().and_then(|ext| ext.to_str()) != Some(format.extension()) {
        return Err(error(
            "INVALID_ARGUMENT",
            format!("--output must end with .{}", format.extension()),
        ));
    }
    if fs::symlink_metadata(path).is_ok() {
        return Err(error(
            "OUTPUT_EXISTS",
            "The output already exists; choose a new path",
        ));
    }
    let parent = path
        .parent()
        .filter(|path| !path.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    if !parent.is_dir() {
        return Err(error(
            "INVALID_ARGUMENT",
            "The output parent directory must exist",
        ));
    }
    Ok(())
}

fn export(args: &CaptionExport) -> Result<Value> {
    validate_destination(&args.output, args.subtitle_format)?;
    let snapshot = Snapshot::load(&args.target.project_path)?;
    let segments = snapshot.segments()?;
    let mut settings = snapshot.settings();
    if let Some(mode) = args.display_mode {
        settings.display_mode = mode.into();
    }
    let coverage = validate_mode(&segments, &settings)?;
    let format = match args.subtitle_format {
        FileFormat::Srt => SubtitleFormat::Srt,
        FileFormat::Vtt => SubtitleFormat::Vtt,
    };
    let content = cap_project::export_subtitles(&segments, &settings, format);
    let cues = content
        .lines()
        .filter(|line| line.contains(" --> "))
        .count();
    if cues == 0 {
        return Err(error(
            "NO_CAPTIONS",
            "No timed captions are available to export",
        ));
    }
    if !args.dry_run {
        let temporary = TemporaryFile::write(&args.output, content.as_bytes())?;
        fs::hard_link(&temporary.path, &args.output).map_err(|cause| {
            if cause.kind() == std::io::ErrorKind::AlreadyExists {
                error(
                    "OUTPUT_EXISTS",
                    "The output already exists; choose a new path",
                )
            } else {
                io_error(cause)
            }
        })?;
    }
    Ok(json!({
        "schemaVersion": 1, "operation": "captions.export", "projectPath": args.target.project_path,
        "revision": snapshot.revision(), "outputPath": args.output, "dryRun": args.dry_run,
        "written": !args.dry_run, "subtitleFormat": args.subtitle_format.extension(),
        "displayMode": settings.display_mode, "cueCount": cues, "coverage": coverage,
        "content": content,
    }))
}

impl StylePatch {
    fn apply(&self, settings: &mut CaptionSettings) -> Result<()> {
        for (name, value, min, max) in [
            ("size", self.size.map(|v| v as f32), 12., 100.),
            (
                "translation-gap",
                self.translation_gap.map(|v| v as f32),
                0.,
                200.,
            ),
            (
                "bottom-margin",
                self.bottom_margin.map(|v| v as f32),
                0.,
                400.,
            ),
            (
                "translation-size",
                self.translation_size.map(|v| v as f32),
                12.,
                100.,
            ),
            (
                "background-opacity",
                self.background_opacity.map(|v| v as f32),
                0.,
                100.,
            ),
            ("outline-width", self.outline_width, 0., 20.),
            (
                "shadow-opacity",
                self.shadow_opacity.map(|v| v as f32),
                0.,
                100.,
            ),
            ("shadow-size", self.shadow_size, 0., 30.),
        ] {
            if value.is_some_and(|value| !value.is_finite() || !(min..=max).contains(&value)) {
                return Err(error(
                    "INVALID_ARGUMENT",
                    format!("--{name} must be between {min} and {max}"),
                ));
            }
        }
        for (name, value) in [
            ("color", &self.color),
            ("outline-color", &self.outline_color),
            ("shadow-color", &self.shadow_color),
        ] {
            if value.as_ref().is_some_and(|value| {
                value.len() != 7
                    || !value.starts_with('#')
                    || !value.as_bytes()[1..].iter().all(u8::is_ascii_hexdigit)
            }) {
                return Err(error(
                    "INVALID_ARGUMENT",
                    format!("--{name} must use #RRGGBB"),
                ));
            }
        }
        for (name, value) in [
            ("font", &self.font),
            ("translation-font", &self.translation_font),
        ] {
            if value.as_ref().is_some_and(|font| font.trim().is_empty()) {
                return Err(error(
                    "INVALID_ARGUMENT",
                    format!("--{name} cannot be blank"),
                ));
            }
        }
        let before = serde_json::to_value(&*settings).map_err(io_error)?;
        if let Some(value) = self.enabled {
            settings.enabled = value;
        }
        if let Some(value) = self.display_mode {
            settings.display_mode = value.into();
        }
        if let Some(value) = &self.font {
            settings.set_source_font(value.clone());
        }
        if let Some(value) = self.size {
            settings.set_source_font_size(value);
        }
        if let Some(value) = &self.translation_font {
            settings.set_translation_font(value.clone());
        }
        if let Some(value) = self.translation_size {
            settings.set_translation_font_size(value);
        }
        if let Some(value) = self.translation_gap {
            settings.translation_gap = Some(value);
        }
        if let Some(value) = self.bottom_margin {
            settings.bottom_margin = Some(value);
        }
        if let Some(value) = &self.color {
            settings.color = value.clone();
        }
        if let Some(value) = self.background_opacity {
            settings.background_opacity = value;
        }
        if let Some(value) = self.outline {
            settings.outline = value;
        }
        if let Some(value) = self.outline_width {
            settings.outline_width = value;
        }
        if let Some(value) = &self.outline_color {
            settings.outline_color = value.clone();
        }
        if let Some(value) = self.shadow {
            settings.shadow = value;
        }
        if let Some(value) = self.shadow_opacity {
            settings.shadow_opacity = value;
        }
        if let Some(value) = self.shadow_size {
            settings.shadow_size = value;
        }
        if let Some(value) = &self.shadow_color {
            settings.shadow_color = value.clone();
        }
        if serde_json::to_value(&*settings).map_err(io_error)? != before {
            settings.preset = "custom".to_string();
        }
        Ok(())
    }
}

fn style(args: &CaptionStyle) -> Result<Value> {
    let mut snapshot = Snapshot::load(&args.target.project_path)?;
    let old_revision = snapshot.revision();
    if args
        .expected_revision
        .as_ref()
        .is_some_and(|expected| *expected != old_revision)
    {
        return Err(error(
            "REVISION_CONFLICT",
            "The project changed; read captions again before applying this edit",
        ));
    }
    let before = snapshot.settings();
    let mut after = before.clone();
    args.patch.apply(&mut after)?;
    let coverage = if args.patch.display_mode.is_some() {
        Some(validate_mode(&snapshot.segments()?, &after)?)
    } else {
        None
    };
    let before_json = serde_json::to_value(&before).map_err(io_error)?;
    let after_json = serde_json::to_value(&after).map_err(io_error)?;
    let changed = before_json != after_json;
    if snapshot.raw.get("captions").is_none_or(Value::is_null) {
        snapshot.raw["captions"] = json!({"segments": [], "settings": {}, "sourceTimed": false});
    }
    if snapshot.raw["captions"].get("settings").is_none() {
        snapshot.raw["captions"]["settings"] = json!({});
    }
    for (key, value) in after_json
        .as_object()
        .ok_or_else(|| error("INVALID_PROJECT", "Invalid caption settings"))?
    {
        if before_json.get(key) != Some(value) {
            if key == "translationStyle"
                && let Some(fields) = value.as_object()
                && let Some(existing) = snapshot.raw["captions"]["settings"][key].as_object_mut()
            {
                existing.extend(fields.clone());
            } else {
                snapshot.raw["captions"]["settings"][key] = value.clone();
            }
        }
    }
    let bytes = serde_json::to_vec_pretty(&snapshot.raw).map_err(io_error)?;
    if changed && !args.dry_run {
        let destination = snapshot.path.join("project-config.json");
        write_checked(&destination, snapshot.bytes.as_deref(), &bytes)?;
    }
    Ok(json!({
        "schemaVersion": 1, "operation": "captions.style", "projectPath": args.target.project_path,
        "revision": old_revision, "resultRevision": if changed { revision(&bytes) } else { snapshot.revision() },
        "dryRun": args.dry_run, "changed": changed, "written": changed && !args.dry_run,
        "before": before, "after": after, "coverage": coverage,
    }))
}

impl CaptionArgs {
    pub fn run(self, json_output: bool) -> std::result::Result<(), String> {
        let (format, result) = match self.command {
            CaptionCommand::Get(args) => (resolve_format(json_output, args.format), get(&args)),
            CaptionCommand::Export(args) => (
                resolve_format(json_output, args.target.format),
                export(&args),
            ),
            CaptionCommand::Style(args) => (
                resolve_format(json_output, args.target.format),
                style(&args),
            ),
        };
        match result {
            Ok(value) => {
                if format == OutputFormat::Json {
                    write_json(&value)
                } else {
                    println!(
                        "{}",
                        serde_json::to_string_pretty(&value).map_err(|cause| cause.to_string())?
                    );
                    Ok(())
                }
            }
            Err(cause) => {
                if format == OutputFormat::Json {
                    write_json(
                        &json!({"error": cause.message, "code": cause.code, "retryable": cause.retryable()}),
                    )?;
                }
                Err(cause.message)
            }
        }
    }
}
