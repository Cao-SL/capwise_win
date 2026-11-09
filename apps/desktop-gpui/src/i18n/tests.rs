use super::*;

#[test]
fn english_is_the_default_locale() {
    assert_eq!(Locale::ALL[index()], Locale::En);
    assert_eq!(Locale::En.code(), "en");
    assert_eq!(Locale::from_code("en"), Some(Locale::En));
}

/// Strings Capwise added rather than moved out of Cap's code: there is no
/// literal they replaced.
const CAPWISE_KEYS: &[&str] = &[
    "ai.",
    "asr.",
    "brand.",
    "dialog.teleprompter.assist.",
    "dialog.teleprompter.follow.",
    "editor.assets.",
    "editor.cover.",
    "editor.review.",
    "editor.setup.",
    "editor.title_card_library.",
    "editor.title_cards.",
    "editor.translation.",
    "editor.voiceover.",
    "menu.file.open_dialog_failed",
    "menu.file.open_failed_title",
    "menu.file.open_instant_missing",
    "menu.file.open_missing",
    "menu.file.open_unreadable",
    "menu.file.open_unsupported",
    "menu.window.recorder",
    "settings.ai.",
    "settings.cover.",
    "settings.about.",
    "settings.general.recording.auto_captions.",
    "settings.nav.about",
    "settings.review.",
    "settings.stock.",
    "settings.workflow.",
    "transcription.qwen",
];

pub(super) fn is_capwise_key(key: &str) -> bool {
    CAPWISE_KEYS.iter().any(|prefix| key.starts_with(prefix))
}

const FORMATTED: &[&str] = &[
    "tray.version",
    "settings.general.excluded_windows.missing_body",
    "settings.general.self_host.confirm_change",
    "menu.update.badge_progress",
    "menu.update.badge_version",
    "editor.assistant.custom_model",
];

#[test]
fn capwise_key_groups_are_present_and_do_not_overlap() {
    for prefix in CAPWISE_KEYS {
        assert!(KEYS.iter().any(|key| key.starts_with(prefix)), "{prefix}");
    }
    for key in KEYS {
        assert!(
            CAPWISE_KEYS
                .iter()
                .filter(|prefix| key.starts_with(**prefix))
                .count()
                <= 1,
            "overlapping Capwise groups: {key}"
        );
    }
}

#[test]
fn english_matches_the_literals_it_replaced() {
    let app_name = crate::menus::APP_NAME;
    let pairs: &[(&str, String)] = &[
        (tr::common::add(), "Add".into()),
        (tr::common::cancel(), "Cancel".into()),
        (tr::common::ok(), "OK".into()),
        (tr::common::reset(), "Reset".into()),
        (tr::common::reset_to_default(), "Reset to Default".into()),
        (tr::common::save(), "Save".into()),
        (tr::menu::app::about(), format!("About {app_name}")),
        (tr::menu::app::hide(), format!("Hide {app_name}")),
        (tr::menu::app::hide_others(), "Hide Others".into()),
        (tr::menu::app::quit(), format!("Quit {app_name}")),
        (tr::menu::app::services(), "Services".into()),
        (tr::menu::app::settings(), "Settings…".into()),
        (tr::menu::file::close_window(), "Close Window".into()),
        (tr::menu::file::open_file(), "Open File…".into()),
        (tr::menu::file::open_prompt(), "Open .cap Project".into()),
        (tr::menu::file::open_recent(), "Open Recent".into()),
        (tr::menu::file::import_media(), "Import Media…".into()),
        (tr::menu::file::title(), "File".into()),
        (tr::menu::edit::copy(), "Copy".into()),
        (tr::menu::edit::cut(), "Cut".into()),
        (tr::menu::edit::paste(), "Paste".into()),
        (tr::menu::edit::redo(), "Redo".into()),
        (tr::menu::edit::select_all(), "Select All".into()),
        (tr::menu::edit::title(), "Edit".into()),
        (tr::menu::edit::undo(), "Undo".into()),
        (
            tr::menu::view::enter_full_screen(),
            "Enter Full Screen".into(),
        ),
        (tr::menu::view::title(), "View".into()),
        (tr::menu::window::close_window(), "Close Window".into()),
        (tr::menu::window::minimize(), "Minimize".into()),
        (tr::menu::window::title(), "Window".into()),
        (tr::menu::window::zoom(), "Zoom".into()),
        (tr::menu::help::title(), "Help".into()),
        (tr::menu::help::feedback(), "Give Feedback…".into()),
        (tr::menu::help::changelog(), "Changelog".into()),
        (
            tr::menu::about::licensed(),
            "Licensed under AGPL-3.0".into(),
        ),
        (tr::menu::about::website(), "Website".into()),
        (tr::menu::about::commit(), "Commit".into()),
        (tr::menu::help::documentation(), "Documentation".into()),
        (tr::menu::update::check(), "Check for Update".into()),
        (tr::menu::update::download(), "Download update…".into()),
        (tr::menu::update::checking(), "Checking for Update…".into()),
        (
            tr::menu::update::downloading(),
            "Downloading update…".into(),
        ),
        (tr::menu::update::restart(), "Restart to update…".into()),
        (tr::tray::import_media(), "Import Media...".into()),
        (tr::tray::no_recent_items(), "No recent items".into()),
        (tr::tray::recording_mode(), "Recording Mode".into()),
        (tr::tray::open_main_window(), "Open Main Window".into()),
        (tr::tray::previous(), "Previous".into()),
        (tr::tray::quit(), "Quit Capwise".into()),
        (tr::tray::record_area(), "Record Area".into()),
        (tr::tray::record_display(), "Record Display".into()),
        (tr::tray::record_window(), "Record Window".into()),
        (
            tr::tray::request_permissions(),
            "Request Permissions".into(),
        ),
        (tr::tray::screenshot_area(), "Screenshot Area".into()),
        (tr::tray::screenshot_display(), "Screenshot Display".into()),
        (tr::tray::screenshot_window(), "Screenshot Window".into()),
        (tr::tray::select_mode(), "Select Mode".into()),
        (tr::tray::settings(), "Settings".into()),
        (
            tr::tray::tooltip_recording(),
            "Capwise - Stop Recording".into(),
        ),
        (
            tr::tray::view_all_recordings(),
            "View all recordings".into(),
        ),
        (tr::tray::mode::instant(), "Instant".into()),
        (tr::tray::mode::screenshot(), "Screenshot".into()),
        (tr::tray::mode::studio(), "Studio".into()),
        (tr::settings::nav::automations(), "Automations".into()),
        (tr::settings::nav::changelog(), "Changelog".into()),
        (tr::settings::nav::cli(), "CLI".into()),
        (tr::settings::nav::experimental(), "Experimental".into()),
        (tr::settings::nav::feedback(), "Feedback".into()),
        (tr::settings::nav::general(), "General".into()),
        (tr::settings::nav::integrations(), "Integrations".into()),
        (tr::settings::nav::license(), "Plan & license".into()),
        (tr::settings::nav::quality(), "Recording & Export".into()),
        (tr::settings::nav::recordings(), "Projects".into()),
        (tr::settings::nav::screenshots(), "Screenshots".into()),
        (tr::settings::nav::shortcuts(), "Shortcuts".into()),
        (tr::settings::nav::transcription(), "AI models".into()),
        (
            tr::settings::general::appearance::description(),
            "Match Capwise to your system theme or pick a fixed look.".into(),
        ),
        (
            tr::settings::general::appearance::title(),
            "Appearance".into(),
        ),
        (tr::settings::general::appearance::system(), "System".into()),
        (tr::settings::general::appearance::light(), "Light".into()),
        (tr::settings::general::appearance::dark(), "Dark".into()),
        (
            tr::settings::general::app::description(),
            "Choose how Capwise shows up on your system.".into(),
        ),
        (tr::settings::general::app::title(), "App".into()),
        (
            tr::settings::general::app::dock_icon::description(),
            "Keep Capwise in the dock even when no windows are open.".into(),
        ),
        (
            tr::settings::general::app::dock_icon::label(),
            "Always show dock icon".into(),
        ),
        (
            tr::settings::general::app::notifications::description(),
            "Show notifications for clipboard copies, saved files, and more. \
                             You may need to allow Capwise in your system's notification settings."
                .into(),
        ),
        (
            tr::settings::general::app::notifications::label(),
            "System notifications".into(),
        ),
        (
            tr::settings::general::recording::description(),
            "Behaviour while you record and after you stop.".into(),
        ),
        (
            tr::settings::general::recording::title(),
            "Recording".into(),
        ),
        (
            tr::settings::general::recording::countdown::description(),
            "Wait before the recording starts.".into(),
        ),
        (
            tr::settings::general::recording::countdown::label(),
            "Countdown".into(),
        ),
        (
            tr::settings::general::recording::countdown::off(),
            "Off".into(),
        ),
        (
            tr::settings::general::recording::countdown::seconds_10(),
            "10 seconds".into(),
        ),
        (
            tr::settings::general::recording::countdown::seconds_3(),
            "3 seconds".into(),
        ),
        (
            tr::settings::general::recording::countdown::seconds_5(),
            "5 seconds".into(),
        ),
        (
            tr::settings::general::recording::confirm_without_mic::description(),
            "Require confirmation when no microphone is selected or the \
                             selected microphone is unavailable."
                .into(),
        ),
        (
            tr::settings::general::recording::confirm_without_mic::label(),
            "Confirm before recording without a microphone".into(),
        ),
        (
            tr::settings::general::recording::studio_sound::description(),
            "Clean up microphone audio automatically. You can still turn it \
                             off for any recording in the editor."
                .into(),
        ),
        (
            tr::settings::general::recording::studio_sound::label(),
            "Studio Sound on new recordings".into(),
        ),
        (
            tr::settings::general::recording::main_window_start::description(),
            "What happens to the main window once a recording begins.".into(),
        ),
        (
            tr::settings::general::recording::main_window_start::label(),
            "Main window when recording starts".into(),
        ),
        (
            tr::settings::general::recording::window_capture::description(),
            "Focused window records only the selected window and follows it when it moves. \
             Centering is optional. Screen region also records overlapping windows and floating \
             panels. Instant with a camera uses screen region."
                .into(),
        ),
        (
            tr::settings::general::recording::window_capture::label(),
            "Window capture style".into(),
        ),
        (
            tr::settings::general::recording::post_deletion::description(),
            "Whether the recording window should reopen.".into(),
        ),
        (
            tr::settings::general::recording::post_deletion::label(),
            "After deleting a recording".into(),
        ),
        (
            tr::settings::general::recording::crash_recovery::description(),
            "Record in fragments that can be recovered after a crash or power \
                             loss. Slightly larger files during capture."
                .into(),
        ),
        (
            tr::settings::general::recording::crash_recovery::label(),
            "Crash-recoverable recording".into(),
        ),
        (
            tr::settings::general::recording::custom_cursor::description(),
            "Capture cursor state separately so you can adjust size and \
                             smoothing in the editor."
                .into(),
        ),
        (
            tr::settings::general::recording::custom_cursor::label(),
            "Custom cursor capture (Studio)".into(),
        ),
        (
            tr::settings::general::recording::auto_zoom::description(),
            "Automatically add zoom segments around mouse clicks while recording.".into(),
        ),
        (
            tr::settings::general::recording::auto_zoom::label(),
            "Auto zoom on clicks".into(),
        ),
        (
            tr::settings::general::recording::default_zoom::description(),
            "Zoom level for newly created and auto-generated zoom segments.".into(),
        ),
        (
            tr::settings::general::recording::default_zoom::label(),
            "Default zoom amount".into(),
        ),
        (
            tr::settings::general::recording::capture_keyboard::description(),
            "Record key presses so you can add keyboard overlays in the editor.".into(),
        ),
        (
            tr::settings::general::recording::capture_keyboard::label(),
            "Capture keyboard presses".into(),
        ),
        (
            tr::settings::general::recording::notch_overlay::description(),
            "Automatically restores the notch for new screen and area \
                             recordings when the selected region contains the complete notch. \
                             External displays, partial areas, and window recordings are left \
                             alone. Each recording can override it in the editor."
                .into(),
        ),
        (
            tr::settings::general::recording::notch_overlay::label(),
            "Draw the MacBook notch on screen recordings".into(),
        ),
        (
            tr::settings::general::recording::marker_sound::label(),
            "Play a sound for markers".into(),
        ),
        (
            tr::settings::general::recording::marker_sound::description(),
            "Plays a short sound when you add a marker while recording. Your \
                             microphone or system audio can pick it up."
                .into(),
        ),
        (
            tr::settings::general::recording::max_fps::description(),
            "Maximum framerate for screen capture.".into(),
        ),
        (
            tr::settings::general::recording::max_fps::description_high(),
            "Maximum framerate for screen capture. Higher values may cause \
                             drops or increased CPU usage on some systems."
                .into(),
        ),
        (
            tr::settings::general::recording::max_fps::fps_120(),
            "120 FPS".into(),
        ),
        (
            tr::settings::general::recording::max_fps::fps_24(),
            "24 FPS".into(),
        ),
        (
            tr::settings::general::recording::max_fps::fps_25(),
            "25 FPS".into(),
        ),
        (
            tr::settings::general::recording::max_fps::fps_30(),
            "30 FPS".into(),
        ),
        (
            tr::settings::general::recording::max_fps::fps_60(),
            "60 FPS (Recommended)".into(),
        ),
        (
            tr::settings::general::recording::max_fps::label(),
            "Max capture framerate".into(),
        ),
        (
            tr::settings::general::storage::choose_folder(),
            "Change…".into(),
        ),
        (
            tr::settings::general::storage::default_location(),
            "Default location".into(),
        ),
        (
            tr::settings::general::storage::description(),
            "Where Capwise saves your recordings.".into(),
        ),
        (tr::settings::general::storage::title(), "Storage".into()),
        (
            tr::settings::general::project_name::available_placeholders(),
            "Available placeholders".into(),
        ),
        (
            tr::settings::general::project_name::description(),
            "Template used for new recordings and exported files.".into(),
        ),
        (
            tr::settings::general::project_name::title(),
            "Default project name".into(),
        ),
        (
            tr::settings::general::project_name::placeholders::date_time(),
            "Date & time".into(),
        ),
        (
            tr::settings::general::project_name::placeholders::date_values(),
            "the recording's date".into(),
        ),
        (
            tr::settings::general::project_name::placeholders::mode_values(),
            "\"studio\", \"instant\", or \"screenshot\"".into(),
        ),
        (
            tr::settings::general::project_name::placeholders::random(),
            "Random".into(),
        ),
        (
            tr::settings::general::project_name::placeholders::random_values(),
            "10 filename-safe hexadecimal characters; use {random:N} for 1–32.".into(),
        ),
        (
            tr::settings::general::project_name::placeholders::recording_mode(),
            "Recording mode".into(),
        ),
        (
            tr::settings::general::project_name::placeholders::recording_mode_values(),
            "\"Studio\", \"Instant\", or \"Screenshot\"".into(),
        ),
        (
            tr::settings::general::project_name::placeholders::target(),
            "Target".into(),
        ),
        (
            tr::settings::general::project_name::placeholders::target_kind_values(),
            "\"Display\", \"Window\", or \"Area\"".into(),
        ),
        (
            tr::settings::general::project_name::placeholders::target_name_values(),
            "Monitor name or window title.".into(),
        ),
        (
            tr::settings::general::project_name::placeholders::time_values(),
            "the recording's time".into(),
        ),
        (
            tr::settings::general::excluded_windows::description(),
            "Hide windows from recordings.".into(),
        ),
        (
            tr::settings::general::excluded_windows::empty(),
            "No windows are currently excluded.".into(),
        ),
        (
            tr::settings::general::excluded_windows::missing_title(),
            "Recommended Capwise windows are not excluded".into(),
        ),
        (
            tr::settings::general::excluded_windows::restore(),
            "Restore".into(),
        ),
        (
            tr::settings::general::excluded_windows::title(),
            "Excluded windows".into(),
        ),
        (
            tr::settings::general::updates::description(),
            "Choose which Capwise builds you receive.".into(),
        ),
        (
            tr::settings::general::updates::nightly_description(),
            "The newest builds, updated automatically in the background when you're \
         not recording or exporting. May be unstable."
                .into(),
        ),
        (
            tr::settings::general::updates::nightly_switch_back(),
            "Switching back to Stable will return you to the latest \
                                     stable version, which may be older than your current build."
                .into(),
        ),
        (
            tr::settings::general::updates::stable_description(),
            "Versioned releases (recommended)".into(),
        ),
        (tr::settings::general::updates::title(), "Updates".into()),
        (
            tr::settings::general::updates::download_policy::label(),
            "Update downloads".into(),
        ),
        (
            tr::settings::general::updates::download_policy::description(),
            "Choose whether Capwise downloads updates automatically.".into(),
        ),
        (
            tr::settings::general::updates::download_policy::manual(),
            "Ask first".into(),
        ),
        (
            tr::settings::general::updates::download_policy::auto(),
            "Auto".into(),
        ),
        (
            tr::settings::general::updates::download_policy::never(),
            "Never".into(),
        ),
        (
            tr::settings::general::updates::channel::description(),
            "Which release channel Capwise updates from.".into(),
        ),
        (
            tr::settings::general::updates::channel::label(),
            "Update channel".into(),
        ),
        (
            tr::settings::general::self_host::description(),
            "Only change this if you are running your own instance of Capwise Web.".into(),
        ),
        (
            tr::settings::general::self_host::server_url(),
            "Capwise Server URL".into(),
        ),
        (
            tr::settings::general::self_host::title(),
            "Self-host".into(),
        ),
        (tr::settings::general::self_host::update(), "Update".into()),
        (tr::settings::general::privacy::title(), "Privacy".into()),
        (
            tr::settings::general::privacy::telemetry::description(),
            "Capwise uses anonymous telemetry to improve reliability and fix bugs. We \
                         never collect recording contents, window titles, file paths, or \
                         personal information."
                .into(),
        ),
        (
            tr::settings::general::privacy::telemetry::label(),
            "Share anonymous telemetry".into(),
        ),
        (tr::menu::file::new_recording(), "New Recording".into()),
        (tr::settings::ai_tabs::recognition(), "Subtitle Recognition".into()),
        (tr::settings::ai_tabs::content(), "Content Generation".into()),
        (tr::settings::ai_tabs::cover(), "Cover Generation".into()),
        (tr::settings::ai_tabs::narration(), "Speech Synthesis".into()),
        (tr::settings::speech::title(), "Doubao Speech Synthesis".into()),
        (tr::settings::speech::description(), "Generate narration from the CLI using this model and voice. The API key is stored in the macOS Keychain.".into()),
        (tr::settings::speech::key(), "Doubao Speech API Key".into()),
        (tr::settings::speech::key_hint(), "Paste a key, or leave blank to keep the saved key".into()),
        (tr::settings::speech::model(), "Model resource ID".into()),
        (tr::settings::speech::voice(), "Voice ID".into()),
        (tr::settings::speech::voice_list(), "Voice catalog".into()),
        (tr::settings::speech::preview(), "Listen to sample".into()),
        (tr::settings::speech::working(), "Working…".into()),
        (tr::settings::speech::sample(), "Hi, this is a sample of your Capwise narration voice.".into()),
        (tr::settings::speech::preview_done(), "Sample played.".into()),
        (tr::settings::speech::saved(), "Saved. Capwise CLI will use this model and voice.".into()),
        (tr::settings::speech::save_failed(), "Could not save the speech settings. Try again.".into()),
        (tr::settings::speech::key_required(), "Enter a Doubao Speech API key.".into()),
        (tr::settings::speech::invalid_settings(), "Check the model resource ID and voice ID.".into()),
        (tr::settings::speech::play_failed(), "Could not play the sample. Check the audio output.".into()),
        (tr::settings::export_folder::save_failed(), "Could not save the export folder. Please try again.".into()),
        (tr::settings::export_folder::title(), "Export Folder".into()),
        (tr::settings::export_folder::description(), "Choose the default folder for videos, subtitles and covers. You can change it when exporting.".into()),
        (tr::settings::export_folder::default_location(), "Use the last system save location".into()),
        (tr::settings::agents::title(), "Local AI agents (ACP)".into()),
        (tr::settings::agents::description(), "Connect an installed agent using its existing login. Suggestions are reviewed in the editor before applying.".into()),
        (tr::settings::agents::save_failed(), "Could not save the agent.".into()),
        (tr::settings::agents::invalid_command(), "Choose an executable using an absolute path. Arguments must be a JSON array of strings.".into()),
        (tr::settings::agents::launch_failed(), "Could not start this agent. Check its installation and command.".into()),
        (tr::settings::agents::protocol_error(), "This agent returned an unsupported ACP response. Check its adapter version.".into()),
        (tr::settings::agents::disconnected(), "The agent disconnected before completing the request.".into()),
        (tr::settings::agents::login_required(), "Sign in using this agent’s CLI, then reconnect.".into()),
        (tr::settings::agents::timeout(), "The agent did not respond in time.".into()),
        (tr::settings::agents::cancelled(), "The agent cancelled this request.".into()),
        (tr::settings::agents::empty_reply(), "The agent returned no text.".into()),
        (tr::settings::agents::connect(), "Connect and get models".into()),
        (tr::settings::agents::refresh(), "Refresh models".into()),
        (tr::settings::agents::remove(), "Disconnect".into()),
        (tr::settings::agents::detect(), "Detect installed agents".into()),
        (tr::settings::agents::not_found(), "Not detected".into()),
        (tr::settings::agents::connected(), "Connected".into()),
        (tr::settings::agents::custom(), "Add custom ACP agent".into()),
        (tr::settings::agents::name(), "Display name".into()),
        (tr::settings::agents::command(), "Executable path".into()),
        (tr::settings::agents::args(), "Arguments (JSON array)".into()),
        (tr::settings::agents::connecting(), "Connecting…".into()),
        (tr::editor::workbench::captions_hidden(), "Captions are hidden in the preview and export.".into()),
        (tr::editor::workbench::content(), "Content".into()),
        (tr::editor::workbench::translation(), "Translation".into()),
        (tr::editor::workbench::previous(), "Previous".into()),
        (tr::editor::workbench::next(), "Next".into()),
        (tr::editor::workbench::back_audio(), "Back to audio".into()),
        (tr::editor::workbench::added_audio(), "Added audio".into()),
        (tr::editor::workbench::audio_empty(), "Record a voiceover or add music using the buttons above.".into()),
        (tr::editor::workbench::adjust(), "Adjust".into()),
        (tr::editor::workbench::recognize_again(), "Recognize again".into()),
        (tr::editor::workbench::text_video_edit(), "Edit video by text".into()),
        (tr::editor::workbench::video_edit_hint(), "Deleting text here also cuts the corresponding video.".into()),
        (tr::editor::workbench::caption_empty(), "Generate captions from your recording to start editing.".into()),
        (tr::editor::workbench::adjust_style(), "Edit style".into()),
        (tr::editor::workbench::original_audio(), "Original recording audio".into()),
        (tr::editor::workbench::no_microphone(), "This recording has no microphone audio.".into()),
        (tr::editor::workbench::voice_enhance(), "Enhance voice".into()),
        (tr::editor::workbench::search_safe_hint(), "Find and replace changes caption text only. Your video stays intact.".into()),
        (tr::editor::workbench::setup_ai(), "Set up AI proofreading".into()),
        (tr::editor::workbench::more_actions(), "More actions".into()),
        (tr::editor::workbench::show_captions(), "Show captions".into()),
        (tr::editor::workbench::my_styles(), "My styles".into()),
        (tr::editor::assistant::no_changes(), "No matching content needs changing. Your project is unchanged.".into()),
        (tr::editor::assistant::favorites(), "Favorites".into()),
        (tr::editor::assistant::skills(), "Skills".into()),
        (tr::editor::assistant::skill_search(), "Search local skills…".into()),
        (tr::editor::assistant::skill_notice(), "The selected skill is sent to your model. Capwise previews and applies edits using its guidance; skill scripts are not executed.".into()),
        (tr::editor::assistant::skill_none(), "No skill".into()),
        (tr::editor::assistant::skill_import(), "Import SKILL.md".into()),
        (tr::editor::assistant::skill_loading(), "Loading skills…".into()),
        (tr::editor::assistant::skill_empty(), "No matching skills. Import a SKILL.md file.".into()),
        (tr::editor::assistant::skill_invalid(), "Cannot read skill. Choose a valid SKILL.md file (up to 64 KB).".into()),
        (tr::editor::assistant::welcome(), "What would you like to create?".into()),
        (tr::editor::assistant::copy_reply(), "Copy response".into()),
        (tr::editor::assistant::details(), "View details".into()),
        (tr::editor::assistant::edit_request(), "Edit and retry".into()),
        (tr::editor::assistant::send_shortcut(), "⌘ Enter to send".into()),
        (tr::editor::assistant::caption_edits(), "Caption text changes".into()),
        (tr::editor::assistant::translate(), "Translate captions".into()),
        (tr::editor::assistant::suggest_rewrite(), "Rewrite captions".into()),
        (tr::editor::assistant::suggest_fillers(), "Remove filler words".into()),
        (tr::editor::assistant::model_search(), "Search, or type a model ID…".into()),
        (tr::editor::assistant::model_settings(), "Model settings".into()),
        (tr::editor::assistant::model_empty(), "No matching models".into()),
        (tr::editor::assistant::default_agent(), "Agent default".into()),
        (tr::editor::assistant::new_chat(), "New conversation".into()),
        (tr::editor::assistant::working(), "Preparing an edit proposal…".into()),
        (tr::editor::assistant::suggest_cover(), "Design a cover".into()),
        (tr::editor::assistant::title(), "AI assistant".into()),
        (tr::editor::assistant::notice(), "Your request, timeline settings and some captions go to the selected model.".into()),
        (tr::editor::assistant::placeholder(), "Describe an edit or a cover idea…".into()),
        (tr::editor::assistant::configure(), "Configure a content generation model in Settings.".into()),
        (tr::editor::assistant::request_limit(), "Enter a request of up to 8,000 bytes.".into()),
        (tr::editor::assistant::invalid(), "The model returned an invalid plan after automatic correction. Your project is unchanged. Retry or switch models.".into()),
        (tr::editor::assistant::cut_blocked(), "This cut conflicts with a transition, title card or the last remaining clip. Choose another range.".into()),
        (tr::editor::assistant::failed(), "The request failed. Please try again.".into()),
        (tr::editor::assistant::changed(), "The project has changed. Generate a new plan before applying.".into()),
        (tr::editor::assistant::applied(), "Changes applied. Use Undo to restore the previous version.".into()),
        (tr::editor::assistant::apply(), "Apply changes".into()),
        (tr::editor::assistant::send(), "Send".into()),
        (tr::editor::assistant::stop(), "Stop".into()),
        (tr::editor::assistant::duration(), "Duration".into()),
        (tr::editor::assistant::cut(), "Remove from clip".into()),
        (tr::editor::assistant::zoom(), "Add zoom".into()),
        (tr::editor::assistant::audio(), "Audio gain".into()),
        (tr::editor::assistant::camera(), "Camera".into()),
        (tr::editor::assistant::captions(), "Captions".into()),
        (tr::editor::assistant::visible(), "Show".into()),
        (tr::editor::assistant::hidden(), "Hide".into()),
        (tr::editor::assistant::cover_settings(), "Edit cover".into()),
        (tr::editor::assistant::cover_working(), "Making the cover…".into()),
        (tr::editor::assistant::regenerate(), "Make another".into()),
        (tr::editor::stickers::title(), "Stickers".into()),
        (tr::editor::stickers::tooltip(), "Insert an emoji or icon".into()),
        (tr::editor::stickers::search(), "Search, e.g. rocket, arrow, check".into()),
        (tr::editor::stickers::emoji(), "Emoji".into()),
        (tr::editor::stickers::icons(), "Icons".into()),
        (tr::editor::stickers::thin(), "Thin".into()),
        (tr::editor::stickers::regular(), "Regular".into()),
        (tr::editor::stickers::bold(), "Bold".into()),
        (tr::editor::stickers::empty(), "Nothing found. Try arrow, heart or warning.".into()),
    ];
    for (translated, literal) in pairs {
        assert_eq!(*translated, literal.as_str());
    }
    let plain_keys = KEYS
        .iter()
        .filter(|key| {
            !key.starts_with(NEW_IN_THE_LANGUAGE_PICKER)
                && !super::surface_tests::is_main_surface_key(key)
                && !super::settings_surface_tests::is_settings_surface_key(key)
                && !super::dialogs_surface_tests::is_dialogs_surface_key(key)
                && !super::editor_panels_surface_tests::is_editor_panels_surface_key(key)
                && !super::editor_shell_surface_tests::is_editor_shell_surface_key(key)
                && !key.starts_with(super::messages::KEY_PREFIX)
                && !is_capwise_key(key)
        })
        .count()
        - FORMATTED.len();
    assert_eq!(
        pairs.len(),
        plain_keys,
        "every plain key has a literal above"
    );
    for key in FORMATTED {
        assert!(KEYS.contains(key), "{key}");
    }
}

const NEW_IN_THE_LANGUAGE_PICKER: &str = "settings.general.language.";

#[test]
fn appearance_labels_follow_language_changes_without_changing_saved_values() {
    use crate::store::{AppTheme, SettingsEnum};

    const CHILD: &str = "CAP_GPUI_TEST_APPEARANCE_LABELS";
    if std::env::var_os(CHILD).is_none() {
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "i18n::tests::appearance_labels_follow_language_changes_without_changing_saved_values",
                "--nocapture",
            ])
            .env(CHILD, "1")
            .env_remove(LOCALE_ENV)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        return;
    }

    for (locale, labels) in [
        (Locale::En, ["System", "Light", "Dark"]),
        (Locale::ZhCn, ["跟随系统", "浅色", "深色"]),
        (Locale::Ja, ["システム", "ライト", "ダーク"]),
        (Locale::Fr, ["Système", "Clair", "Sombre"]),
        (Locale::Es, ["Sistema", "Claro", "Oscuro"]),
        (Locale::De, ["System", "Hell", "Dunkel"]),
        (Locale::Hi, ["सिस्टम", "लाइट", "डार्क"]),
        (Locale::En, ["System", "Light", "Dark"]),
    ] {
        init(Some(locale.code()));
        for ((theme, label), saved) in AppTheme::ALL
            .iter()
            .zip(labels)
            .zip(["system", "light", "dark"])
        {
            assert_eq!(theme.label(), label, "{} {saved}", locale.code());
            assert_eq!(theme.as_json(), saved);
            assert_eq!(AppTheme::from_json(saved), Some(*theme));
        }
    }
}

#[test]
fn language_picker_english() {
    assert_eq!(tr::settings::general::language::title(), "Language");
    assert_eq!(
        tr::settings::general::language::description(),
        "Choose the language Capwise is shown in."
    );
    assert_eq!(tr::settings::general::language::label(), "App language");
    assert_eq!(
        tr::settings::general::language::system_default("Deutsch"),
        "System default (Deutsch)"
    );
    assert_eq!(
        KEYS.iter()
            .filter(|key| key.starts_with(NEW_IN_THE_LANGUAGE_PICKER))
            .count(),
        4
    );
}

#[test]
fn system_tags_resolve_to_catalogs() {
    for (tag, expected) in [
        ("en-US", Some(Locale::En)),
        ("en_GB.UTF-8", Some(Locale::En)),
        ("fr-CA", Some(Locale::Fr)),
        ("de_DE@euro", Some(Locale::De)),
        ("es-419", Some(Locale::Es)),
        ("hi-IN", Some(Locale::Hi)),
        ("ja-JP", Some(Locale::Ja)),
        ("JA", Some(Locale::Ja)),
        ("zh", Some(Locale::ZhCn)),
        ("zh-CN", Some(Locale::ZhCn)),
        ("zh-SG", Some(Locale::ZhCn)),
        ("zh-Hans", Some(Locale::ZhCn)),
        ("zh-Hans-HK", Some(Locale::ZhCn)),
        ("zh_CN.UTF-8", Some(Locale::ZhCn)),
        ("zh-Hant", None),
        ("zh-Hant-CN", None),
        ("zh-TW", None),
        ("zh-HK", None),
        ("pt-BR", None),
        ("C", None),
        ("", None),
    ] {
        assert_eq!(match_tag(tag), expected, "{tag:?}");
    }
}

#[test]
fn traditional_chinese_falls_through_to_the_next_language() {
    let tags = |tags: &[&str]| tags.iter().map(|tag| tag.to_string()).collect::<Vec<_>>();
    assert_eq!(first_supported(tags(&["zh-Hant-TW", "ja-JP"])), Locale::Ja);
    assert_eq!(first_supported(tags(&["zh-Hant-TW"])), Locale::En);
    assert_eq!(first_supported(tags(&["pt-BR", "de-AT", "fr"])), Locale::De);
    assert_eq!(first_supported(Vec::new()), Locale::En);
}

#[test]
fn system_default_follows_the_system_language_and_a_choice_overrides_it() {
    assert_eq!(
        resolve_preference(Preference::System, || Locale::Ja),
        Locale::Ja
    );
    assert_eq!(
        resolve_preference(Preference::Fixed(Locale::De), || Locale::Ja),
        Locale::De
    );
}

#[test]
fn stored_preferences_round_trip_and_unknown_values_follow_the_system() {
    assert_eq!(Preference::from_setting(None), Preference::System);
    assert_eq!(Preference::from_setting(Some("system")), Preference::System);
    assert_eq!(Preference::from_setting(Some("pt-BR")), Preference::System);
    assert_eq!(Preference::System.setting(), "system");
    for locale in Locale::ALL {
        let preference = Preference::Fixed(locale);
        assert_eq!(
            Preference::from_setting(Some(preference.setting())),
            preference
        );
    }
    let options: Vec<Preference> = Preference::options().collect();
    assert_eq!(options.len(), Locale::ALL.len() + 1);
    assert_eq!(options[0], Preference::System);
    assert_eq!(options[1], Preference::Fixed(Locale::En));
}

#[test]
fn picker_lists_endonyms_and_pseudo_only_in_debug_builds() {
    let endonyms: Vec<&str> = Locale::ALL.iter().map(|locale| locale.endonym()).collect();
    let mut expected = vec![
        "English",
        "简体中文",
        "日本語",
        "Français",
        "Español",
        "Deutsch",
        "हिन्दी",
    ];
    if cfg!(debug_assertions) {
        expected.push("Pseudo");
    }
    assert_eq!(endonyms, expected);
    assert_eq!(
        Locale::from_code("pseudo").is_some(),
        cfg!(debug_assertions)
    );
}

#[test]
fn only_cjk_and_devanagari_catalogs_change_the_font_cascade() {
    let first = |locale| {
        crate::theme::locale_font_fallbacks(locale)
            .map(|fallbacks| fallbacks.fallback_list().to_vec())
    };
    for locale in [Locale::En, Locale::Fr, Locale::Es, Locale::De] {
        assert_eq!(first(locale), None, "{locale:?}");
    }
    assert_eq!(
        first(Locale::Ja).unwrap(),
        ["Hiragino Sans", "Yu Gothic UI", "Noto Sans CJK JP"]
    );
    assert_eq!(
        first(Locale::ZhCn).unwrap(),
        ["PingFang SC", "Microsoft YaHei UI", "Noto Sans CJK SC"]
    );
    assert_eq!(
        first(Locale::Hi).unwrap(),
        ["Kohinoor Devanagari", "Nirmala UI", "Noto Sans Devanagari"]
    );
}

/// Geist has no CJK or Devanagari glyphs, so those clusters are sized the way
/// the system faces set them: ideographs and kana are one em, Devanagari
/// letters a little over half, vowel signs a third or nothing.
fn missing_glyph_width(cluster: &str, size: f32) -> f32 {
    cluster
        .chars()
        .map(|c| match c {
            '\u{0900}'..='\u{0902}'
            | '\u{093A}'
            | '\u{093C}'
            | '\u{0941}'..='\u{0948}'
            | '\u{094D}'
            | '\u{0951}'..='\u{0957}'
            | '\u{0962}'..='\u{0963}' => 0.,
            '\u{0903}'
            | '\u{093B}'
            | '\u{093E}'..='\u{0940}'
            | '\u{0949}'..='\u{094C}'
            | '\u{094E}'..='\u{094F}' => 0.34,
            '\u{0900}'..='\u{097F}' => 0.62,
            _ => 1.,
        })
        .sum::<f32>()
        * size
}

/// Every string with a `max_px` budget, in every catalog, measured with the
/// bundled Geist Medium (the widest weight these labels use). A translation
/// over 92% of the budget fails (the system UI font differs from Geist by a
/// few percent), unless English is already wider, which then is the limit.
#[test]
fn translations_fit_their_width_budgets() {
    use cosmic_text::{
        Attrs, Buffer, Family, FontSystem, Metrics, Shaping, Weight, fontdb::Database,
    };

    let fonts = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("assets/fonts");
    let mut database = Database::new();
    for name in ["Geist.ttf", "Geist-Medium.ttf"] {
        database.load_font_data(std::fs::read(fonts.join(name)).unwrap());
    }
    let mut system = FontSystem::new_with_locale_and_db("en-US".into(), database);
    let attrs = Attrs::new()
        .family(Family::Name("Geist"))
        .weight(Weight::MEDIUM);

    let mut measure = |text: &str, size: f32| {
        let mut buffer = Buffer::new(&mut system, Metrics::new(size, size * 1.25));
        buffer.set_size(&mut system, None, None);
        buffer.set_text(&mut system, text, &attrs, Shaping::Advanced);
        buffer.shape_until_scroll(&mut system, false);
        buffer
            .layout_runs()
            .map(|run| {
                let mut width = 0.;
                let mut missing_cluster = None;
                for glyph in run.glyphs {
                    if glyph.glyph_id != 0 {
                        width += glyph.w;
                    } else if missing_cluster != Some((glyph.start, glyph.end)) {
                        // A shaped syllable can be several .notdef glyphs
                        // that all point at the same cluster.
                        missing_cluster = Some((glyph.start, glyph.end));
                        width += missing_glyph_width(&run.text[glyph.start..glyph.end], size);
                    }
                }
                width
            })
            .fold(0., f32::max)
    };

    let mut measured = 0;
    let mut failures = Vec::new();
    for fit in &FITS {
        // English is what ships today; where it already runs past 92% the
        // translations may not be any wider than it.
        let limit = (fit.max_px * 0.92).max(measure(fit.text[0], fit.size));
        for (locale, text) in Locale::ALL.iter().zip(fit.text).skip(1) {
            if locale.code() == "pseudo" {
                continue;
            }
            let width = measure(text, fit.size);
            measured += 1;
            if width > limit {
                failures.push(format!(
                    "{} `{}` {text:?}: {width:.1}px, limit {limit:.1}px of {}px",
                    locale.code(),
                    fit.key,
                    fit.max_px
                ));
            }
        }
    }
    assert!(measured > 0);
    assert!(
        failures.is_empty(),
        "over 92% of the width budget:\n{}",
        failures.join("\n")
    );
}

#[test]
fn english_messages_format_like_the_literals_they_replaced() {
    let version = "0.6.1";
    assert_eq!(tr::tray::version(version), format!("Capwise v{version}"));
    assert_eq!(
        tr::asr::quota("Doubao Speech", "overdue"),
        "Doubao Speech quota exhausted or account in arrears: overdue"
    );
    assert_eq!(
        tr::asr::failed("Groq", 429, "slow down"),
        "Groq failed (429): slow down"
    );
    assert_eq!(
        tr::asr::connection_failed("OpenAI", "timeout"),
        "Could not reach OpenAI: timeout"
    );
    assert_eq!(
        tr::settings::about::license(2026),
        "© 2026 向阳乔木 · AGPL-3.0 · Based on Cap"
    );
    assert_eq!(tr::settings::ai::ready_ms(812), "Ready · 812 ms");
    assert_eq!(tr::settings::ai::downloading(42), "Downloading 42%");
    let names = "Cap Camera, Cap Settings".to_string();
    assert_eq!(
        tr::settings::general::excluded_windows::missing_body(&names),
        format!(
            "Camera, settings, or recording windows can appear as black \
                                 boxes in screen recordings. Missing: {names}."
        )
    );
    let origin = "https://cap.example.com";
    assert_eq!(
        tr::settings::general::self_host::confirm_change(origin),
        format!(
            "Are you sure you want to change the server URL to '{origin}'? You will need \
                 to sign in again."
        )
    );

    assert_eq!(
        tr::menu::update::badge_progress("0.4.0", 42),
        "v0.4.0 · 42%"
    );
    assert_eq!(tr::menu::update::badge_version("0.4.0"), "v0.4.0");
}

#[test]
fn capwise_product_messages_preserve_arguments() {
    let cases = [
        (
            tr::editor::assistant::custom_model("model-example"),
            "Use custom model: model-example",
        ),
        (
            tr::settings::workflow::delete_title("教程"),
            "Delete “教程”?",
        ),
        (
            tr::settings::workflow::copy_name("教程", 2),
            "教程 · Copy 2",
        ),
        (
            tr::editor::translation::service("DeepSeek"),
            "Translation service: DeepSeek",
        ),
        (
            tr::editor::translation::partial(2, 9),
            "Translated 2/9 captions. Untranslated captions show the original text.",
        ),
        (
            tr::editor::translation::progress(2, 9),
            "Translated 2 of 9 captions",
        ),
        (
            tr::editor::translation::review(2, 9),
            "Review translations · 2/9",
        ),
        (
            tr::editor::review::record_unreadable("permission denied"),
            "Cannot read subtitle status: permission denied",
        ),
        (tr::editor::assets::retry_in(5), "Retry in 5s"),
        (
            tr::editor::assets::drop_at("00:12"),
            "Release at 00:12 · Esc to cancel",
        ),
        (
            tr::editor::assets::add_image("00:12"),
            "Add overlay · 00:12",
        ),
        (
            tr::editor::setup::not_persisted("disk full"),
            "Preparation could not be saved for recovery after restarting: disk full",
        ),
        (
            tr::editor::title_cards::export_size("1920 × 1080"),
            "PNG · 1920 × 1080 pixels · Project aspect ratio",
        ),
        (
            tr::editor::title_cards::export_saved("1920 × 1080", "/exports/封面.png"),
            "Saved 1920 × 1080 PNG: /exports/封面.png",
        ),
        (
            tr::editor::voiceover::position("00:12", 3),
            "Start 00:12 · Audio track 3",
        ),
        (
            tr::editor::voiceover::asset_name("00:12"),
            "Voiceover 00:12",
        ),
        (
            tr::editor::voiceover::recover_count(2),
            "Unfinished takes (2)",
        ),
        (
            tr::editor::title_card_library::details(2.5, "Demo"),
            "2.5 sec · Demo",
        ),
        (
            tr::editor::title_card_library::remove_confirm("Demo"),
            "Remove “Demo”? Projects already using this template will keep their copies.",
        ),
        (
            tr::editor::title_card_library::failed("disk full"),
            "Could not complete this action: disk full",
        ),
        (
            tr::editor::title_card_library::unreadable("invalid image"),
            "Cannot read this template: invalid image. You can remove it and save a new copy.",
        ),
    ];
    for (translated, expected) in cases {
        assert_eq!(translated, expected);
    }
}

#[test]
fn prompt_buttons_keep_their_roles() {
    assert_eq!(
        gpui::PromptButton::ok(tr::common::ok()),
        gpui::PromptButton::from("Ok")
    );
    assert_eq!(
        gpui::PromptButton::cancel(tr::common::cancel()),
        gpui::PromptButton::from("Cancel")
    );
}

#[test]
fn every_key_is_used_outside_the_catalog() {
    let src = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let i18n = src.join("i18n");
    let mut sources = Vec::new();
    let mut pending = vec![src];
    while let Some(dir) = pending.pop() {
        for entry in std::fs::read_dir(&dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                if path != i18n {
                    pending.push(path);
                }
            } else if path.extension().is_some_and(|extension| extension == "rs") {
                sources.push(std::fs::read_to_string(&path).unwrap());
            }
        }
    }
    sources.push(std::fs::read_to_string(i18n.join("messages.rs")).unwrap());
    let unused: Vec<&str> = KEYS
        .iter()
        .copied()
        .filter(|key| {
            !sources
                .iter()
                .any(|source| source_uses_translation(source, key))
        })
        .collect();
    assert!(
        unused.is_empty(),
        "keys in locales/en.toml that no code reads: {unused:?}"
    );
}

fn contains_accessor(source: &str, accessor: &str) -> bool {
    let is_identifier = |character: char| character.is_ascii_alphanumeric() || character == '_';
    source.match_indices(accessor).any(|(at, _)| {
        source[..at]
            .chars()
            .next_back()
            .is_none_or(|previous| !is_identifier(previous))
            && source[at + accessor.len()..]
                .chars()
                .next()
                .is_none_or(|next| !is_identifier(next))
    })
}

fn source_uses_translation(source: &str, key: &str) -> bool {
    let path = key.replace('.', "::");
    if contains_accessor(source, &format!("tr::{path}")) {
        return true;
    }
    if !source.contains("use tr::") {
        return false;
    }
    source.lines().any(|line| {
        let Some(import) = line
            .trim()
            .strip_prefix("use tr::")
            .and_then(|line| line.strip_suffix(';'))
        else {
            return false;
        };
        let (module, alias) = import
            .split_once(" as ")
            .unwrap_or_else(|| (import, import.rsplit("::").next().unwrap_or(import)));
        let Some(suffix) = path
            .strip_prefix(module)
            .and_then(|suffix| suffix.strip_prefix("::"))
        else {
            return false;
        };
        contains_accessor(source, &format!("{alias}::{suffix}"))
    })
}

#[test]
fn translation_usage_resolves_module_aliases_without_accepting_unused_children() {
    let source = "use tr::dialog::teleprompter::assist as copy;\ncopy::write();\ncopy::current();";
    assert!(source_uses_translation(
        source,
        "dialog.teleprompter.assist.write"
    ));
    assert!(source_uses_translation(
        source,
        "dialog.teleprompter.assist.current"
    ));
    assert!(!source_uses_translation(
        source,
        "dialog.teleprompter.assist.restore"
    ));
    assert!(!source_uses_translation(
        "use tr::settings::workflow as text;",
        "settings.workflow.title"
    ));
    assert!(source_uses_translation(
        "use tr::settings::workflow;\nworkflow::title();",
        "settings.workflow.title"
    ));
}

#[test]
fn translation_usage_does_not_leak_aliases_between_files_or_accessor_names() {
    let sources = [
        "use tr::one as text;\ntext::title();",
        "use tr::two as text;\ntext::other();",
    ];
    assert!(
        !sources
            .iter()
            .any(|source| source_uses_translation(source, "two.title"))
    );
    assert!(
        sources
            .iter()
            .any(|source| source_uses_translation(source, "one.title"))
    );
    assert!(!source_uses_translation(
        "tr::one::title_suffix();",
        "one.title"
    ));
    assert!(!source_uses_translation(
        "other_tr::one::title();",
        "one.title"
    ));
    assert!(!source_uses_translation(
        "use tr::one as text;\nother_text::title();",
        "one.title"
    ));
}
