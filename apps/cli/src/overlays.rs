use std::{fs, path::PathBuf};

use cap_project::{
    OverlayImage, OverlayRange, OverlayTrackKind, ProjectConfiguration, RecordingMeta,
    RecordingMetaInner, insert_image_segment, insert_mask_segment, insert_text_segment,
};
use clap::{Args, Subcommand, ValueEnum};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use crate::{
    OutputFormat,
    project_edit::{Result, error, io_error, revision, write_checked_with},
    resolve_format, write_json,
};

#[derive(Args)]
pub struct OverlayArgs {
    #[command(subcommand)]
    command: OverlayCommand,
}

#[derive(Subcommand)]
enum OverlayCommand {
    #[command(about = "Add a text title over the video")]
    Title {
        #[command(flatten)]
        edit: Edit,
        #[arg(long)]
        text: String,
    },
    #[command(about = "Copy a local image into the project and place it on the canvas")]
    Image {
        #[command(flatten)]
        edit: Edit,
        #[arg(long)]
        file: PathBuf,
    },
    #[command(about = "Blur or pixelate a rectangle on the output canvas")]
    Redact {
        #[command(flatten)]
        edit: Edit,
        #[arg(long, help = "Normalized left edge (0–1)")]
        x: f64,
        #[arg(long, help = "Normalized top edge (0–1)")]
        y: f64,
        #[arg(long)]
        width: f64,
        #[arg(long)]
        height: f64,
        #[arg(long, value_enum, default_value = "blur")]
        effect: Effect,
    },
}

#[derive(Args)]
struct Edit {
    project_path: PathBuf,
    #[arg(long, help = "Start in edited output seconds")]
    start: f64,
    #[arg(long, help = "End in edited output seconds")]
    end: f64,
    #[arg(long, help = "Preview without writing the project or an image")]
    dry_run: bool,
    #[arg(
        long,
        required_unless_present = "dry_run",
        help = "Revision from dry-run"
    )]
    expected_revision: Option<String>,
    #[arg(long, value_enum, default_value_t = OutputFormat::Text)]
    format: OutputFormat,
}

#[derive(Clone, Copy, ValueEnum)]
enum Effect {
    Blur,
    Pixelate,
}

impl OverlayCommand {
    fn edit(&self) -> &Edit {
        match self {
            Self::Title { edit, .. } | Self::Image { edit, .. } | Self::Redact { edit, .. } => edit,
        }
    }

    fn apply(&self) -> Result<Value> {
        let edit = self.edit();
        let destination = edit.project_path.join("project-config.json");
        let original = match fs::read(&destination) {
            Ok(bytes) => Some(bytes),
            Err(cause) if cause.kind() == std::io::ErrorKind::NotFound => None,
            Err(cause) => return Err(io_error(cause)),
        };
        let current_revision = revision(original.as_deref().unwrap_or_default());
        if edit
            .expected_revision
            .as_ref()
            .is_some_and(|expected| *expected != current_revision)
        {
            return Err(error(
                "REVISION_CONFLICT",
                "The project changed; preview the edit again",
            ));
        }
        let mut raw: Value = original
            .as_ref()
            .map_or_else(|| Ok(json!({})), |bytes| serde_json::from_slice(bytes))
            .map_err(|cause| error("INVALID_PROJECT", cause.to_string()))?;
        let mut project: ProjectConfiguration = serde_json::from_value(raw.clone())
            .map_err(|cause| error("INVALID_PROJECT", cause.to_string()))?;
        project
            .validate()
            .map_err(|cause| error("INVALID_PROJECT", cause.to_string()))?;
        let meta = RecordingMeta::load_for_project(&edit.project_path)
            .map_err(|cause| error("INVALID_PROJECT", cause.to_string()))?;
        let RecordingMetaInner::Studio(studio) = &meta.inner else {
            return Err(error(
                "INVALID_PROJECT",
                "Overlay editing requires a studio project",
            ));
        };
        let recordings = cap_rendering::ProjectRecordingsMeta::new(&edit.project_path, studio)
            .map_err(|cause| error("MEDIA_UNAVAILABLE", cause))?;
        cap_export::synthesize_default_timeline(&mut project, &recordings);
        if raw["timeline"].is_null() {
            raw["timeline"] = serde_json::to_value(&project.timeline).map_err(io_error)?;
        }
        if !project.overlays_are_layered() {
            project.migrate_overlay_layers();
            if let Some(timeline) = &project.timeline {
                for (kind, index, track, _, _) in timeline.overlay_spans() {
                    let row = raw["timeline"][overlay_field(kind)]
                        .get_mut(index)
                        .ok_or_else(|| error("INVALID_PROJECT", "Missing overlay data"))?;
                    row["track"] = json!(track);
                }
            }
        }
        let aspect = recordings.segments.first().map_or(16. / 9., |segment| {
            let options = cap_rendering::RenderOptions {
                screen_size: cap_project::XY::new(segment.display.width, segment.display.height),
                camera_size: None,
                preserve_screen_alpha: false,
            };
            let (width, height) = cap_rendering::ProjectUniforms::get_base_size(&options, &project);
            f64::from(width.max(1)) / f64::from(height.max(1))
        });
        let timeline = project
            .timeline
            .as_mut()
            .ok_or_else(|| error("MEDIA_UNAVAILABLE", "The project has no video timeline"))?;
        let kind = match self {
            Self::Title { .. } => OverlayTrackKind::Text,
            Self::Image { .. } => OverlayTrackKind::Image,
            Self::Redact { .. } => OverlayTrackKind::Mask,
        };
        let range = OverlayRange::new(timeline, kind, edit.start, edit.end, timeline.duration())
            .map_err(|message| error("INVALID_ARGUMENT", message))?;
        let mut image_asset = None;
        let (operation, index, segment) = match self {
            Self::Title { text, .. } => {
                if text.trim().is_empty() || text.len() > 6000 {
                    return Err(error(
                        "INVALID_ARGUMENT",
                        "Provide title text between 1 and 6000 bytes",
                    ));
                }
                let segment = range.text(text.clone());
                let value = serde_json::to_value(&segment).map_err(io_error)?;
                ("title", insert_text_segment(timeline, segment), value)
            }
            Self::Image { file, .. } => {
                let image = OverlayImage::from_file(file)
                    .map_err(|message| error("INVALID_MEDIA", message))?;
                let mut hash = Sha256::new();
                hash.update(current_revision.as_bytes());
                hash.update(image.bytes());
                let digest = hash.finalize();
                let mut id = [0; 16];
                id.copy_from_slice(&digest[..16]);
                let id = uuid::Uuid::from_bytes(id);
                let segment = range.image(
                    image.relative_path(id),
                    image.name.clone(),
                    image.width,
                    image.height,
                    aspect,
                );
                let value = serde_json::to_value(&segment).map_err(io_error)?;
                image_asset = Some((image, id));
                ("image", insert_image_segment(timeline, segment), value)
            }
            Self::Redact {
                x,
                y,
                width,
                height,
                effect,
                ..
            } => {
                let segment = range
                    .redact(*x, *y, *width, *height, matches!(effect, Effect::Pixelate))
                    .map_err(|message| error("INVALID_ARGUMENT", message))?;
                let value = serde_json::to_value(&segment).map_err(io_error)?;
                ("redact", insert_mask_segment(timeline, segment), value)
            }
        };
        let rows = &mut raw["timeline"][overlay_field(kind)];
        if rows.is_null() {
            *rows = json!([]);
        }
        let rows = rows
            .as_array_mut()
            .ok_or_else(|| error("INVALID_PROJECT", "Expected an overlay array"))?;
        rows.push(segment.clone());
        rows.sort_by(|a, b| {
            let key = |value: &Value| {
                (
                    value["track"].as_u64().unwrap_or_default(),
                    value["start"].as_f64().unwrap_or_default(),
                    value["end"].as_f64().unwrap_or_default(),
                )
            };
            let (at, as_, ae) = key(a);
            let (bt, bs, be) = key(b);
            at.cmp(&bt).then(as_.total_cmp(&bs)).then(ae.total_cmp(&be))
        });
        project.set_overlay_order(project.overlay_tracks());
        let updated = serde_json::to_value(&project).map_err(io_error)?;
        for field in [
            "overlayLayerVersion",
            "overlayOrder",
            "overlayOrderWithShapes",
        ] {
            if let Some(value) = updated.get(field) {
                raw[field] = value.clone();
            } else if let Some(object) = raw.as_object_mut() {
                object.remove(field);
            }
        }
        let bytes = serde_json::to_vec_pretty(&raw).map_err(io_error)?;
        if !edit.dry_run {
            write_checked_with(&destination, original.as_deref(), &bytes, || {
                let Some((image, id)) = image_asset else {
                    return Ok(None);
                };
                let created = image
                    .write_to_project(&edit.project_path, id)
                    .map_err(io_error)?;
                Ok(created.then(|| edit.project_path.join(image.relative_path(id))))
            })?;
        }
        Ok(json!({
            "schemaVersion": 1, "operation": format!("overlay.{operation}"),
            "timeBasis": "edited-output-seconds", "index": index, "overlay": segment,
            "revision": current_revision, "resultRevision": revision(&bytes),
            "dryRun": edit.dry_run, "written": !edit.dry_run,
        }))
    }
}

fn overlay_field(kind: OverlayTrackKind) -> &'static str {
    match kind {
        OverlayTrackKind::Text => "textSegments",
        OverlayTrackKind::Image => "imageSegments",
        OverlayTrackKind::Mask => "maskSegments",
        OverlayTrackKind::Shape => "shapeSegments",
    }
}

impl OverlayArgs {
    pub fn run(self, json_output: bool) -> std::result::Result<(), String> {
        let format = resolve_format(json_output, self.command.edit().format);
        match self.command.apply() {
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
