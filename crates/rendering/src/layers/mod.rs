mod animated_gradient;
mod background;
mod blur;
mod camera;
mod camera3d;
mod captions;
mod click_ripple;
mod color_grade;
mod cursor;
mod display;
mod font;
mod frame;
mod image;
mod keyboard;
mod mask;
mod notch;
mod shape;
mod text;
mod title_card;

use std::sync::OnceLock;

/// Building a `glyphon::FontSystem` scans and parses every installed system font,
/// which costs hundreds of milliseconds to over a second on macOS. cosmic-text
/// explicitly documents that it should be created once and shared. We previously
/// built three of them per `RendererLayers` (text, captions, keyboard) and a fresh
/// `RendererLayers` per editor/screenshot instance, so every open paid that scan
/// several times over.
///
/// Instead, scan the system fonts a single time per process, then cheaply clone the
/// resulting font database (memory-mapped faces are reference counted) for each new
/// `FontSystem`.
pub fn new_font_system() -> glyphon::FontSystem {
    let font_phase = crate::readiness::Phase::start("font.instance");
    let template_phase = crate::readiness::Phase::start("font.template_lookup");
    let (locale, db) = font_template();
    template_phase.finish("returned");

    let result = crate::readiness::measure("font.clone_and_index", || {
        glyphon::FontSystem::new_with_locale_and_db(locale.clone(), db.clone())
    });
    font_phase.finish("returned");
    result
}

pub fn font_family_available(family: &str) -> bool {
    font_template()
        .1
        .faces()
        .any(|face| face.families.iter().any(|(name, _)| name == family))
}

/// Where the fonts the app ships live: `CAPWISE_FONTS_DIR`, or
/// `Contents/Resources/fonts` next to the running executable.
pub fn bundled_font_dirs() -> Vec<std::path::PathBuf> {
    let mut dirs = Vec::new();
    if let Some(dir) = std::env::var_os("CAPWISE_FONTS_DIR") {
        dirs.push(std::path::PathBuf::from(dir));
    }
    if let Ok(exe) = std::env::current_exe()
        && let Some(contents) = exe.parent().and_then(std::path::Path::parent)
    {
        dirs.push(contents.join("Resources").join("fonts"));
    }
    dirs.into_iter().filter(|dir| dir.is_dir()).collect()
}

/// A font family whose names include a Chinese one, for the caption font
/// picker. `family` is the name the renderer matches on.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChineseFontFamily {
    pub family: String,
    pub chinese_name: String,
}

/// Installed and bundled families that name themselves in Chinese, which
/// in practice means they carry Chinese glyphs. Hidden system faces (a
/// leading dot) are left out.
pub fn chinese_font_families() -> Vec<ChineseFontFamily> {
    use glyphon::fontdb::Language;
    let (_, db) = font_template();
    let mut families: Vec<ChineseFontFamily> = Vec::new();
    for face in db.faces() {
        let Some((family, _)) = face.families.first() else {
            continue;
        };
        if family.starts_with('.') || families.iter().any(|known| &known.family == family) {
            continue;
        }
        let chinese = face.families.iter().find(|(_, language)| {
            matches!(
                language,
                Language::Chinese_PeoplesRepublicOfChina
                    | Language::Chinese_Singapore
                    | Language::Chinese_Taiwan
                    | Language::Chinese_HongKongSAR
                    | Language::Chinese_MacaoSAR
            )
        });
        if let Some((chinese_name, _)) = chinese {
            families.push(ChineseFontFamily {
                family: family.clone(),
                chinese_name: chinese_name.clone(),
            });
        }
    }
    families.sort_by(|a, b| a.family.cmp(&b.family));
    families
}

fn font_template() -> &'static (String, glyphon::fontdb::Database) {
    static FONT_TEMPLATE: OnceLock<(String, glyphon::fontdb::Database)> = OnceLock::new();

    FONT_TEMPLATE.get_or_init(|| {
        let init_phase = crate::readiness::Phase::start("font.template_init");
        let font_system = crate::readiness::measure("font.system_scan", glyphon::FontSystem::new);
        let mut db = font_system.db().clone();
        for dir in bundled_font_dirs() {
            db.load_fonts_dir(dir);
        }
        // Pin the generic families to the fonts the editor webview resolves
        // them to. fontdb's stock defaults (e.g. "Arial") often don't match
        // any installed face, in which case cosmic-text silently shapes with
        // an arbitrary fallback font — and canvas overlays measured in the
        // webview no longer match what the renderer draws.
        #[cfg(target_os = "macos")]
        {
            // WKWebView: sans-serif → Helvetica, serif → Times, monospace →
            // Courier.
            db.set_sans_serif_family("Helvetica");
            db.set_serif_family("Times New Roman");
            db.set_monospace_family("Courier New");
        }
        #[cfg(windows)]
        {
            // WebView2 (Chromium): sans-serif → Arial, serif → Times New
            // Roman, monospace → Consolas.
            db.set_sans_serif_family("Arial");
            db.set_serif_family("Times New Roman");
            db.set_monospace_family("Consolas");
        }
        #[cfg(all(unix, not(target_os = "macos")))]
        {
            db.set_sans_serif_family("DejaVu Sans");
            db.set_serif_family("DejaVu Serif");
            db.set_monospace_family("DejaVu Sans Mono");
        }
        let template = (font_system.locale().to_string(), db);
        init_phase.finish("returned");
        template
    })
}

/// Bind groups for full-frame passes that sample one of the render session's
/// two textures. Each is keyed by the uniform buffer and source view it binds,
/// so it is built once per pairing instead of once per frame.
#[derive(Default)]
pub struct SourceBindGroups {
    entries: Vec<(wgpu::Buffer, wgpu::TextureView, wgpu::BindGroup)>,
    #[cfg(test)]
    created: usize,
}

impl SourceBindGroups {
    pub fn get_or_create(
        &mut self,
        buffer: &wgpu::Buffer,
        view: &wgpu::TextureView,
        create: impl FnOnce() -> wgpu::BindGroup,
    ) -> &wgpu::BindGroup {
        let index = match self
            .entries
            .iter()
            .position(|(cached_buffer, cached_view, _)| {
                cached_buffer == buffer && cached_view == view
            }) {
            Some(index) => index,
            None => {
                #[cfg(test)]
                {
                    self.created += 1;
                }
                self.entries.push((buffer.clone(), view.clone(), create()));
                self.entries.len() - 1
            }
        };
        &self.entries[index].2
    }

    /// Drops entries for views the session no longer owns, so a resized
    /// session's old textures are released.
    pub fn retain_views(&mut self, views: [&wgpu::TextureView; 2]) {
        self.entries.retain(|(_, view, _)| views.contains(&view));
    }

    #[cfg(test)]
    pub fn created(&self) -> usize {
        self.created
    }
}

pub use animated_gradient::*;
pub use background::*;
pub use blur::*;
pub use camera::*;
pub use camera3d::*;
pub use captions::*;
pub use click_ripple::*;
pub use color_grade::*;
pub use cursor::*;
pub use display::*;
pub use frame::*;
pub use image::*;
pub use keyboard::*;
pub use mask::*;
pub use notch::*;
pub use shape::*;
pub use text::*;
pub(crate) use title_card::TitleCardLayer;

#[cfg(test)]
mod font_tests {
    use super::*;

    /// Text overlays are measured in the editor webview with the CSS generic
    /// `sans-serif`; the renderer must resolve the same generic to a real
    /// (and matching) font or boxes and line wrapping diverge from the
    /// rendered pixels.
    #[test]
    fn generic_families_resolve_to_real_fonts() {
        let font_system = new_font_system();
        for family in [
            glyphon::fontdb::Family::SansSerif,
            glyphon::fontdb::Family::Serif,
            glyphon::fontdb::Family::Monospace,
        ] {
            for weight in [400u16, 700] {
                let query = glyphon::fontdb::Query {
                    families: &[family],
                    weight: glyphon::fontdb::Weight(weight),
                    ..Default::default()
                };
                let id = font_system.db().query(&query);
                let families = id
                    .and_then(|id| font_system.db().face(id))
                    .map(|face| face.families.clone());
                println!("{family:?} weight {weight}: {families:?}");
                assert!(
                    families.is_some(),
                    "{family:?} (weight {weight}) resolved to no font"
                );
            }
        }
    }
}
