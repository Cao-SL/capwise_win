use windows::{
    Data::Xml::Dom::XmlDocument,
    UI::Notifications::{ToastNotification, ToastNotificationManager},
    core::HSTRING,
};

use super::jump_list::APP_USER_MODEL_ID;

fn escape_xml(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for ch in value.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            // Toast XML rejects most control characters.
            ch if ch.is_control() => {}
            ch => out.push(ch),
        }
    }
    out
}

fn payload(title: &str, body: &str) -> String {
    format!(
        "<toast><visual><binding template=\"ToastGeneric\">\
         <text>{}</text><text>{}</text></binding></visual></toast>",
        escape_xml(title),
        escape_xml(body)
    )
}

/// Show a Windows toast. CreateToastNotifierWithId pins the notification to
/// the same explicit AppUserModelID as the taskbar button, so it works even
/// when the Start-menu shortcut has no System.AppUserModel.ID. Any WinRT
/// failure is a silent no-op: toast delivery is best-effort.
pub fn show(title: &str, body: &str) {
    let result = (|| -> windows::core::Result<()> {
        let xml = XmlDocument::new()?;
        xml.LoadXml(&HSTRING::from(payload(title, body).as_str()))?;
        let notification = ToastNotification::CreateToastNotification(&xml)?;
        let notifier =
            ToastNotificationManager::CreateToastNotifierWithId(&HSTRING::from(APP_USER_MODEL_ID))?;
        notifier.Show(&notification)
    })();
    if let Err(error) = result {
        tracing::debug!(%error, "toast notification unavailable");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_payload_escapes_xml_and_drops_control_characters() {
        let xml = payload("A & B <tag>", "line \"quoted\"\u{0007}end");
        assert!(xml.contains("<text>A &amp; B &lt;tag&gt;</text>"));
        assert!(xml.contains("&quot;quoted&quot;end"));
        assert!(!xml.contains('\u{0007}'));
        assert!(xml.starts_with("<toast><visual>"));
        assert!(xml.ends_with("</toast>"));
    }

    #[test]
    fn the_notifier_uses_the_same_app_id_as_the_jump_list() {
        assert_eq!(APP_USER_MODEL_ID, "com.capwise.app");
    }
}
