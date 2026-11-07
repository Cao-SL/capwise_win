//! Stand-in for `src/screenshot.rs` when the `screenshots` feature is off.

#![allow(dead_code)]

use cap_recording::sources::screen_capture::ScreenCaptureTarget;
use gpui::App;

pub fn take_screenshot(target: ScreenCaptureTarget, _cx: &mut App) {
    tracing::warn!(
        kind = target.kind_str(),
        "screenshot mode is not part of this build"
    );
}
