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
            "mouse" => include_bytes!("../../assets/mouse.svg"),
            "keyboard" => include_bytes!("../../assets/keyboard.svg"),
            "headset" => include_bytes!("../../assets/headset.svg"),
            "gamepad" => include_bytes!("../../assets/gamepad.svg"),
            "bluetooth" => include_bytes!("../../assets/bluetooth.svg"),
            "receiver24g" => include_bytes!("../../assets/receiver24g.svg"),
            "update" => include_bytes!("../../assets/update.svg"),
            "empty" => include_bytes!("../../assets/empty.svg"),
            "edit" => include_bytes!("../../assets/edit.svg"),
            "pin" => include_bytes!("../../assets/pin.svg"),
            _ => return Ok(None),
        })))
    }
    fn list(&self, _: &str) -> gpui::Result<Vec<SharedString>> {
        Ok(vec![
            "refresh".into(),
            "settings".into(),
            "close".into(),
            "scan".into(),
            "mouse".into(),
            "keyboard".into(),
            "headset".into(),
            "gamepad".into(),
            "bluetooth".into(),
            "receiver24g".into(),
            "update".into(),
            "empty".into(),
            "edit".into(),
            "pin".into(),
        ])
    }
}
