use std::{
    fs,
    path::{Path, PathBuf},
};

use cap_project::{
    ProjectConfiguration, RecordingMeta, TextSegment, TitleCardBackground, TitleCardRole,
    TitleCardTemplate,
};
use clap::{Args, Subcommand, ValueEnum};
use image::ImageDecoder;
use serde_json::{Value, json};

use crate::{
    OutputFormat,
    project_edit::{Result, TemporaryFile, error, io_error, revision, write_checked},
    resolve_format, write_json,
};

#[derive(Args)]
pub struct TitleCardArgs {
    #[command(subcommand)]
    command: TitleCardCommand,
}

#[derive(Subcommand)]
enum TitleCardCommand {
    #[command(about = "Read intro/outro cards, their output times, and project revision")]
    Get(Target),
    #[command(about = "Create or patch one title card; preview with --dry-run first")]
    Set(SetCard),
    #[command(about = "Remove a title card and restore the body's output timing")]
    Remove(RemoveCard),
    #[command(about = "Export a title card as lossless PNG using the video's renderer")]
    Export(ExportCard),
}

#[derive(Args)]
struct Target {
    project_path: PathBuf,
    #[arg(long, value_enum, default_value_t = OutputFormat::Text)]
    format: OutputFormat,
}

#[derive(Clone, Copy, ValueEnum)]
enum Role {
    Intro,
    Outro,
}

impl From<Role> for TitleCardRole {
    fn from(role: Role) -> Self {
        match role {
            Role::Intro => Self::Intro,
            Role::Outro => Self::Outro,
        }
    }
}

#[derive(Clone, Copy, ValueEnum)]
enum Template {
    Hero,
    Editorial,
    Split,
}

impl From<Template> for TitleCardTemplate {
    fn from(template: Template) -> Self {
        match template {
            Template::Hero => Self::Hero,
            Template::Editorial => Self::Editorial,
            Template::Split => Self::Split,
        }
    }
}

#[derive(Args)]
struct Edit {
    #[command(flatten)]
    target: Target,
    #[arg(long, value_enum)]
    role: Role,
    #[arg(
        long,
        help = "Preview all timing and card changes without writing any files"
    )]
    dry_run: bool,
    #[arg(
        long,
        required_unless_present = "dry_run",
        help = "Revision from get or dry-run"
    )]
    expected_revision: Option<String>,
}

#[derive(Args)]
struct SetCard {
    #[command(flatten)]
    edit: Edit,
    #[arg(long, help = "Main heading; required when creating a new card")]
    title: Option<String>,
    #[arg(long)]
    subtitle: Option<String>,
    #[arg(long)]
    kicker: Option<String>,
    #[arg(long, value_enum)]
    template: Option<Template>,
    #[arg(long, help = "Hold duration in output seconds, 0.1–30")]
    duration: Option<f64>,
    #[arg(long)]
    font: Option<String>,
    #[arg(long, help = "Heading size at a 1080p reference, 24–240")]
    font_size: Option<f32>,
    #[arg(long, conflicts_with = "background_image", help = "Background #RRGGBB")]
    background_color: Option<String>,
    #[arg(
        long,
        help = "Existing image path relative to the project; never uploads files"
    )]
    background_image: Option<String>,
}

#[derive(Args)]
struct RemoveCard {
    #[command(flatten)]
    edit: Edit,
}

#[derive(Args)]
struct ExportCard {
    #[command(flatten)]
    target: Target,
    #[arg(long, value_enum, default_value = "intro")]
    role: Role,
    #[arg(long, help = "New .png file; existing files are never overwritten")]
    output: PathBuf,
    #[arg(
        long,
        default_value_t = 1920,
        help = "Resolution bound; preserves the project's aspect ratio"
    )]
    width: u32,
    #[arg(
        long,
        default_value_t = 1920,
        help = "Resolution bound; preserves the project's aspect ratio"
    )]
    height: u32,
    #[arg(long, help = "Validate the request without rendering or writing files")]
    dry_run: bool,
    #[arg(long, help = "Optional revision from get; rejects a changed project")]
    expected_revision: Option<String>,
    #[arg(long)]
    force_ffmpeg_decoder: bool,
}

fn validate_export_destination(path: &Path) -> Result<()> {
    if path.extension().and_then(|extension| extension.to_str()) != Some("png") {
        return Err(error("INVALID_ARGUMENT", "--output must end with .png"));
    }
    match fs::symlink_metadata(path) {
        Ok(_) => {
            return Err(error(
                "OUTPUT_EXISTS",
                "The output already exists; choose a new path",
            ));
        }
        Err(cause) if cause.kind() == std::io::ErrorKind::NotFound => {}
        Err(cause) => return Err(io_error(cause)),
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

async fn export(args: &ExportCard) -> Result<Value> {
    validate_export_destination(&args.output)?;
    if args.width == 0
        || args.height == 0
        || args.width > 16_384
        || args.height > 16_384
        || u64::from(args.width) * u64::from(args.height) > 40_000_000
    {
        return Err(error(
            "INVALID_ARGUMENT",
            "Resolution bounds must be nonzero, at most 16384 per side and 40 million pixels",
        ));
    }
    let snapshot = Snapshot::load(&args.target)?;
    let current_revision = revision(&snapshot.bytes);
    if args
        .expected_revision
        .as_ref()
        .is_some_and(|expected| *expected != current_revision)
    {
        return Err(error(
            "REVISION_CONFLICT",
            "The project changed; read it again before exporting",
        ));
    }
    let role = args.role.into();
    let segment = snapshot.project.title_card(role).ok_or_else(|| {
        error(
            "NO_TITLE_CARD",
            "No title card exists for the selected role",
        )
    })?;
    let mut validation = snapshot.project.clone();
    if validation
        .normalize_title_cards()
        .map_err(|cause| error("INVALID_PROJECT", cause.to_string()))?
        || !segment.enabled
    {
        return Err(error(
            "INVALID_PROJECT",
            "The title card timing is inconsistent; update it with title-card set before exporting",
        ));
    }
    let frame_number = (segment.start * 30.0).ceil();
    let frame_time = frame_number / 30.0;
    if frame_number > f64::from(u32::MAX) || frame_time >= segment.end {
        return Err(error(
            "INVALID_PROJECT",
            "The title card does not contain a renderable frame",
        ));
    }
    let card_id = segment.title_card.as_ref().unwrap().id.clone();
    let mut result = json!({
        "schemaVersion": 1, "operation": "title-card.export", "projectPath": args.target.project_path,
        "revision": current_revision, "outputPath": args.output, "dryRun": args.dry_run,
        "written": false, "imageFormat": "png", "role": role, "cardId": card_id,
        "frameTime": frame_time, "resolutionBound": {"width": args.width, "height": args.height},
    });
    if !args.dry_run {
        let mut project = snapshot.project;
        project.migrate_text_settings();
        let image = cap_export::preview::render_still_with_config(
            args.target.project_path.clone(),
            project,
            (frame_number + 0.5) / 30.0,
            cap_project::XY::new(args.width, args.height),
            args.force_ffmpeg_decoder,
        )
        .await
        .map_err(|cause| error("RENDER_FAILED", cause.to_string()))?;
        let temporary = TemporaryFile::write(&args.output, &image.png)?;
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
        result["written"] = true.into();
        result["width"] = image.width.into();
        result["height"] = image.height.into();
        result["bytes"] = image.png.len().into();
    }
    Ok(result)
}

struct Snapshot {
    bytes: Vec<u8>,
    raw: Value,
    project: ProjectConfiguration,
}

impl Snapshot {
    fn load(target: &Target) -> Result<Self> {
        RecordingMeta::load_for_project(&target.project_path)
            .map_err(|cause| error("INVALID_PROJECT", cause.to_string()))?;
        let bytes = fs::read(target.project_path.join("project-config.json")).map_err(io_error)?;
        let raw: Value = serde_json::from_slice(&bytes)
            .map_err(|cause| error("INVALID_PROJECT", cause.to_string()))?;
        let project: ProjectConfiguration = serde_json::from_value(raw.clone())
            .map_err(|cause| error("INVALID_PROJECT", cause.to_string()))?;
        project
            .validate()
            .map_err(|cause| error("INVALID_PROJECT", cause.to_string()))?;
        Ok(Self {
            bytes,
            raw,
            project,
        })
    }

    fn check_revision(&self, edit: &Edit) -> Result<()> {
        if edit
            .expected_revision
            .as_ref()
            .is_some_and(|expected| *expected != revision(&self.bytes))
        {
            return Err(error(
                "REVISION_CONFLICT",
                "The project changed; read it again before applying this edit",
            ));
        }
        Ok(())
    }
}

fn cards(project: &ProjectConfiguration) -> Value {
    json!({
        "intro": project.title_card(TitleCardRole::Intro),
        "outro": project.title_card(TitleCardRole::Outro),
        "outputDuration": project.timeline.as_ref().map(|timeline| timeline.duration()),
    })
}

fn get(target: &Target) -> Result<Value> {
    let snapshot = Snapshot::load(target)?;
    Ok(json!({
        "schemaVersion": 1, "operation": "title-card.get", "projectPath": target.project_path,
        "revision": revision(&snapshot.bytes), "timeBasis": "edited-output-seconds",
        "cards": cards(&snapshot.project),
    }))
}

fn set(args: &SetCard) -> Result<Value> {
    let snapshot = Snapshot::load(&args.edit.target)?;
    snapshot.check_revision(&args.edit)?;
    let role = args.edit.role.into();
    let mut segment = if let Some(segment) = snapshot.project.title_card(role) {
        segment.clone()
    } else {
        let title = args
            .title
            .as_deref()
            .ok_or_else(|| error("INVALID_ARGUMENT", "--title is required for a new card"))?;
        let mut segment = TextSegment::new_title_card(role, title.to_string());
        let identity = format!("{}:{role:?}", revision(&snapshot.bytes));
        segment.title_card.as_mut().unwrap().id = revision(identity.as_bytes())[..32].to_string();
        segment
    };
    if let Some(title) = &args.title {
        if title.trim().is_empty() {
            return Err(error("INVALID_ARGUMENT", "--title cannot be blank"));
        }
        segment.content = title.clone();
    }
    if let Some(duration) = args.duration {
        segment.end = segment.start + duration;
    }
    if let Some(font) = &args.font {
        if font.trim().is_empty() || font.len() > 256 || font.chars().any(char::is_control) {
            return Err(error(
                "INVALID_ARGUMENT",
                "--font must be a valid font family name",
            ));
        }
        segment.font_family = font.clone();
    }
    if let Some(size) = args.font_size {
        if !size.is_finite() || !(24.0..=240.0).contains(&size) {
            return Err(error(
                "INVALID_ARGUMENT",
                "--font-size must be between 24 and 240",
            ));
        }
        segment.font_size = size;
    }
    let card = segment.title_card.as_mut().unwrap();
    if let Some(subtitle) = &args.subtitle {
        card.subtitle = subtitle.clone();
    }
    if let Some(kicker) = &args.kicker {
        card.kicker = kicker.clone();
    }
    if let Some(template) = args.template {
        card.template = template.into();
    }
    if let Some(color) = &args.background_color {
        card.background = TitleCardBackground::Solid {
            color: color.clone(),
        };
    }
    if let Some(path) = &args.background_image {
        card.background = TitleCardBackground::ProjectImage {
            path: path.clone(),
            fit: cap_project::TitleCardImageFit::Cover,
            focus: cap_project::XY::new(0.5, 0.5),
            dim: 0.35,
        };
        card.validate()
            .map_err(|cause| error("INVALID_ARGUMENT", cause.to_string()))?;
        let project_root = fs::canonicalize(&args.edit.target.project_path).map_err(io_error)?;
        let image_path = fs::canonicalize(project_root.join(path)).map_err(io_error)?;
        if !image_path.starts_with(&project_root) || !image_path.is_file() {
            return Err(error(
                "INVALID_ARGUMENT",
                "The background image must be inside the project",
            ));
        }
        if fs::metadata(&image_path).map_err(io_error)?.len() > 256 * 1024 * 1024 {
            return Err(error(
                "INVALID_MEDIA",
                "The background image exceeds 256 MiB",
            ));
        }
        let mut reader = image::ImageReader::open(&image_path)
            .and_then(image::ImageReader::with_guessed_format)
            .map_err(|cause| error("INVALID_MEDIA", cause.to_string()))?;
        let mut limits = image::Limits::default();
        limits.max_image_width = Some(32_768);
        limits.max_image_height = Some(32_768);
        limits.max_alloc = Some(256 * 1024 * 1024);
        reader.limits(limits);
        let decoder = reader
            .into_decoder()
            .map_err(|cause| error("INVALID_MEDIA", cause.to_string()))?;
        let (width, height) = decoder.dimensions();
        if width == 0 || height == 0 || u64::from(width) * u64::from(height) > 40_000_000 {
            return Err(error(
                "INVALID_MEDIA",
                "Background images must be nonempty and at most 40 million pixels",
            ));
        }
        drop(
            image::DynamicImage::from_decoder(decoder)
                .map_err(|cause| error("INVALID_MEDIA", cause.to_string()))?,
        );
    }
    let mut after = snapshot.project.clone();
    after
        .upsert_title_card(segment)
        .map_err(|cause| error("INVALID_ARGUMENT", cause.to_string()))?;
    finish_edit(snapshot, after, &args.edit, "title-card.set")
}

fn remove(args: &RemoveCard) -> Result<Value> {
    let snapshot = Snapshot::load(&args.edit.target)?;
    snapshot.check_revision(&args.edit)?;
    let mut after = snapshot.project.clone();
    after
        .remove_title_card(args.edit.role.into())
        .map_err(|cause| error("INVALID_ARGUMENT", cause.to_string()))?;
    finish_edit(snapshot, after, &args.edit, "title-card.remove")
}

fn finish_edit(
    snapshot: Snapshot,
    after: ProjectConfiguration,
    edit: &Edit,
    operation: &str,
) -> Result<Value> {
    let before_json = serde_json::to_value(&snapshot.project).map_err(io_error)?;
    let after_json = serde_json::to_value(&after).map_err(io_error)?;
    let changed = before_json != after_json;
    let mut raw = snapshot.raw.clone();
    merge_changed_fields(&mut raw, &before_json, &after_json, "")?;
    let bytes = serde_json::to_vec_pretty(&raw).map_err(io_error)?;
    if changed && !edit.dry_run {
        write_checked(
            &edit.target.project_path.join("project-config.json"),
            Some(&snapshot.bytes),
            &bytes,
        )?;
    }
    Ok(json!({
        "schemaVersion": 1, "operation": operation, "projectPath": edit.target.project_path,
        "revision": revision(&snapshot.bytes),
        "resultRevision": if changed { revision(&bytes) } else { revision(&snapshot.bytes) },
        "dryRun": edit.dry_run, "changed": changed, "written": changed && !edit.dry_run,
        "before": cards(&snapshot.project), "after": cards(&after),
    }))
}

fn merge_changed_fields(raw: &mut Value, before: &Value, after: &Value, field: &str) -> Result<()> {
    if before == after {
        return Ok(());
    }
    match (before, after) {
        (Value::Object(old), Value::Object(new)) => {
            if !raw.is_object() {
                *raw = json!({});
            }
            for key in old.keys().filter(|key| !new.contains_key(*key)) {
                raw.as_object_mut().unwrap().remove(key);
            }
            for (key, value) in new {
                if old.get(key) != Some(value) {
                    merge_changed_fields(
                        &mut raw[key],
                        old.get(key).unwrap_or(&Value::Null),
                        value,
                        key,
                    )?;
                }
            }
        }
        (Value::Array(old), Value::Array(new)) if field == "textSegments" => {
            let original = raw.as_array().cloned().unwrap_or_default();
            let mut ordinary = old
                .iter()
                .enumerate()
                .filter(|(_, item)| item.get("titleCard").is_none());
            let mut patched = Vec::with_capacity(new.len());
            for item in new {
                let previous = if let Some(id) = item.pointer("/titleCard/id") {
                    old.iter()
                        .enumerate()
                        .find(|(_, value)| value.pointer("/titleCard/id") == Some(id))
                } else {
                    ordinary.next()
                };
                if let Some((index, previous)) = previous {
                    let mut value = original.get(index).cloned().unwrap_or(Value::Null);
                    merge_changed_fields(&mut value, previous, item, "")?;
                    patched.push(value);
                } else {
                    patched.push(item.clone());
                }
            }
            *raw = Value::Array(patched);
        }
        (Value::Array(old), Value::Array(new)) if old.len() == new.len() => {
            if !raw.is_array() {
                *raw = Value::Array(old.clone());
            }
            let raw_items = raw.as_array_mut().unwrap();
            if raw_items.len() != new.len() {
                return Err(error("INVALID_PROJECT", "Unexpected project array shape"));
            }
            if field == "sceneSegments" {
                let mut ordered = raw_items
                    .iter()
                    .enumerate()
                    .map(|(index, item)| {
                        item.get("start")
                            .and_then(Value::as_f64)
                            .map(|start| (index, start))
                            .ok_or_else(|| error("INVALID_PROJECT", "Invalid scene start time"))
                    })
                    .collect::<Result<Vec<_>>>()?;
                ordered.sort_by(|left, right| left.1.total_cmp(&right.1));
                for ((index, _), (before, after)) in ordered.into_iter().zip(old.iter().zip(new)) {
                    merge_changed_fields(&mut raw_items[index], before, after, "")?;
                }
                return Ok(());
            }
            for ((value, before), after) in raw_items.iter_mut().zip(old).zip(new) {
                merge_changed_fields(value, before, after, "")?;
            }
        }
        _ => *raw = after.clone(),
    }
    Ok(())
}

impl TitleCardArgs {
    pub fn renders_image(&self) -> bool {
        matches!(&self.command, TitleCardCommand::Export(args) if !args.dry_run)
    }

    pub async fn run(self, json_output: bool) -> std::result::Result<(), String> {
        let (format, result) = match self.command {
            TitleCardCommand::Get(args) => (resolve_format(json_output, args.format), get(&args)),
            TitleCardCommand::Set(args) => (
                resolve_format(json_output, args.edit.target.format),
                set(&args),
            ),
            TitleCardCommand::Remove(args) => (
                resolve_format(json_output, args.edit.target.format),
                remove(&args),
            ),
            TitleCardCommand::Export(args) => (
                resolve_format(json_output, args.target.format),
                export(&args).await,
            ),
        };
        match result {
            Ok(value) if format == OutputFormat::Json => write_json(&value),
            Ok(value) => {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&value).map_err(|cause| cause.to_string())?
                );
                Ok(())
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
