//! Stand-in for `src/screenshot_editor.rs` when the `screenshots` feature is
//! off. No path resolves to a screenshot bundle, so the window is never opened.

#![allow(dead_code)]

use std::cell::RefCell;
use std::convert::Infallible;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use gpui::{App, Context, IntoElement, Render, Window, WindowHandle};

pub const SCREENSHOT_EDITOR_WIDTH: f32 = 1240.;
pub const SCREENSHOT_EDITOR_HEIGHT: f32 = 800.;
pub const SCREENSHOT_EDITOR_MIN_WIDTH: f32 = 800.;
pub const SCREENSHOT_EDITOR_MIN_HEIGHT: f32 = 600.;

pub fn resolve_bundle(_path: &Path) -> Option<PathBuf> {
    None
}

#[derive(Default)]
pub struct PendingConfigSave;

impl PendingConfigSave {
    pub fn discard(&mut self) {}

    pub fn flush(&mut self) {}
}

pub struct ScreenshotEditorWindow {
    never: Infallible,
}

impl ScreenshotEditorWindow {
    pub(crate) fn export_in_flight(&self) -> bool {
        match self.never {}
    }

    pub fn new(_bundle: PathBuf, _window: &mut Window, _cx: &mut Context<Self>) -> Self {
        unreachable!("resolve_bundle never yields a bundle without the screenshots feature")
    }

    pub fn pending_save(&self) -> Rc<RefCell<PendingConfigSave>> {
        match self.never {}
    }
}

impl Render for ScreenshotEditorWindow {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        gpui::Empty
    }
}

pub fn load_screenshot_project(
    _bundle: PathBuf,
    _handle: WindowHandle<ScreenshotEditorWindow>,
    _cx: &mut App,
) {
}
