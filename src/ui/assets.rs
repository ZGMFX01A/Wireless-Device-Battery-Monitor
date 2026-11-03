use gpui::{AssetSource, SharedString};
use std::borrow::Cow;
pub struct Icons;
impl AssetSource for Icons {
    fn load(&self, path: &str) -> gpui::Result<Option<Cow<'static, [u8]>>> {
        Ok(Some(Cow::Borrowed(match path {
            "refresh" => include_bytes!("../../assets/refresh.svg"),
            "settings" => include_bytes!("../../assets/settings.svg"),
            "close" => include_bytes!("../../assets/close.svg"),
            "scan" => include_bytes!("../../assets/scan.svg"),
            "keyboard" => include_bytes!("../../assets/keyboard.svg"),
            "bluetooth" => include_bytes!("../../assets/bluetooth.svg"),
            "update" => include_bytes!("../../assets/update.svg"),
            "empty" => include_bytes!("../../assets/empty.svg"),
            "edit" => include_bytes!("../../assets/edit.svg"),
            _ => return Ok(None),
        })))
    }
    fn list(&self, _: &str) -> gpui::Result<Vec<SharedString>> {
        Ok(vec![
            "refresh".into(),
            "settings".into(),
            "close".into(),
            "scan".into(),
            "keyboard".into(),
            "bluetooth".into(),
            "update".into(),
            "empty".into(),
            "edit".into(),
        ])
    }
}
