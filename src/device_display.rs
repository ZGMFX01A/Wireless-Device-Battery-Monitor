// SPDX-FileCopyrightText: 2026 ZGMFX01A
// SPDX-License-Identifier: LicenseRef-Wireless-Device-Battery-Monitor
// See LICENSE in the repository root for the full terms.

//! Pure presentation decisions shared by native UI surfaces.
use crate::view::strings::{Locale, label};
use battery_protocol::{
    BatteryCapability, ConnectionKind, Descriptor, DeviceCategory, GenericName, Kind,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DeviceIcon {
    Mouse,
    Keyboard,
    Headset,
    Gamepad,
    Bluetooth,
    Receiver24g,
}

pub fn is_hyperx(device: &Descriptor, source: Option<battery_protocol::Source>) -> bool {
    source == Some(battery_protocol::Source::HyperX)
        || device.identity.as_ref().is_some_and(|identity| {
            identity.model_source == Some(battery_protocol::ModelSource::CatalogRule)
                && identity.model_key.as_deref().is_some_and(|key| {
                    key.starts_with("legacy:03f0:") || key.starts_with("legacy:0951:")
                })
        })
}

/// 根据有效采样来源或已确认目录身份判定 Turtle Beach 品牌徽章，
/// 不仅凭厂商字符串或未确认 VID 推断。
pub fn is_turtle_beach(device: &Descriptor, source: Option<battery_protocol::Source>) -> bool {
    source == Some(battery_protocol::Source::TurtleBeach)
        || device.identity.as_ref().is_some_and(|identity| {
            identity.model_source == Some(battery_protocol::ModelSource::CatalogRule)
                && identity
                    .model_key
                    .as_deref()
                    .is_some_and(|key| key.starts_with("legacy:10f5:"))
        })
}

/// 仅凭已确认 `scyrox:` 模型、Scyrox 2.4G 接收器或 `395A` 端点 Scyrox 采样显示 Scyrox 徽章，
/// 严禁从共享 VID（`3554` / `2FE3`）或未确认产品字符串推断品牌。
pub fn is_scyrox(device: &Descriptor, source: Option<battery_protocol::Source>) -> bool {
    let confirmed_model = device.identity.as_ref().is_some_and(|identity| {
        matches!(
            identity.model_source,
            Some(
                battery_protocol::ModelSource::CatalogRule
                    | battery_protocol::ModelSource::ProtocolIdentity
            )
        ) && identity
            .model_key
            .as_deref()
            .is_some_and(|key| key.starts_with("scyrox:"))
    });
    let scyrox_receiver = matches!(
        &device.generic_name,
        Some(GenericName::Receiver24g {
            brand: Some(brand),
            ..
        }) if brand == "Scyrox"
    );
    let own_vid_sample =
        source == Some(battery_protocol::Source::Scyrox) && device.id.starts_with("hid:395a:");
    confirmed_model || scyrox_receiver || own_vid_sample
}

pub fn device_icon(device: &Descriptor) -> Option<DeviceIcon> {
    let Some(identity) = &device.identity else {
        return Some(match device.kind {
            Kind::Mouse => DeviceIcon::Mouse,
            Kind::Keyboard => DeviceIcon::Keyboard,
            Kind::Bluetooth => DeviceIcon::Bluetooth,
            Kind::Audio => DeviceIcon::Headset,
            Kind::Gamepad => DeviceIcon::Gamepad,
        });
    };
    if identity.battery == BatteryCapability::Absent {
        return None;
    }
    if identity.model_key.is_some()
        && let Some(icon) = category_icon(identity.category)
    {
        return Some(icon);
    }
    match identity.connection {
        ConnectionKind::Rf24 => Some(DeviceIcon::Receiver24g),
        ConnectionKind::Bluetooth => {
            Some(category_icon(identity.category).unwrap_or(DeviceIcon::Bluetooth))
        }
        ConnectionKind::Usb | ConnectionKind::Unknown => category_icon(identity.category),
    }
}

fn category_icon(category: DeviceCategory) -> Option<DeviceIcon> {
    match category {
        DeviceCategory::Mouse => Some(DeviceIcon::Mouse),
        DeviceCategory::Keyboard => Some(DeviceIcon::Keyboard),
        DeviceCategory::Headset => Some(DeviceIcon::Headset),
        DeviceCategory::Gamepad => Some(DeviceIcon::Gamepad),
        DeviceCategory::Unknown => None,
    }
}

/// Localize only a generic name explicitly marked by the identity resolver.
/// Exact model names, system-provided names, and user aliases remain unchanged.
pub fn display_name(device: &Descriptor, locale: Locale) -> String {
    match &device.generic_name {
        None => device.name.clone(),
        Some(GenericName::UnknownDevice) => unknown_device_name(device, locale),
        Some(GenericName::Receiver24g { brand, channel }) => {
            receiver_name(locale, brand.as_deref(), *channel)
        }
    }
}

fn unknown_device_name(device: &Descriptor, locale: Locale) -> String {
    let Some(identity) = &device.identity else {
        return label(locale, "display.device.unknown").into();
    };
    match identity.connection {
        ConnectionKind::Rf24 => receiver_name(locale, None, Some(identity.category)),
        ConnectionKind::Bluetooth => category_icon(identity.category).map_or_else(
            || label(locale, "display.device.bluetooth").into(),
            |icon| icon_accessible_name(locale, icon).into(),
        ),
        ConnectionKind::Usb | ConnectionKind::Unknown => category_icon(identity.category)
            .map_or_else(
                || label(locale, "display.device.unknown").into(),
                |icon| icon_accessible_name(locale, icon).into(),
            ),
    }
}

fn receiver_name(locale: Locale, brand: Option<&str>, channel: Option<DeviceCategory>) -> String {
    let receiver = label(locale, "display.device.receiver24g");
    let mut name = brand.map_or_else(|| receiver.into(), |brand| format!("{brand} {receiver}"));
    let channel = match channel {
        Some(DeviceCategory::Mouse) => Some(label(locale, "display.channel.mouse")),
        Some(DeviceCategory::Keyboard) => Some(label(locale, "display.channel.keyboard")),
        Some(DeviceCategory::Headset | DeviceCategory::Gamepad | DeviceCategory::Unknown)
        | None => None,
    };
    if let Some(channel) = channel {
        name.push_str(" (");
        name.push_str(channel);
        name.push(')');
    }
    name
}

pub fn icon_accessible_name(locale: Locale, icon: DeviceIcon) -> &'static str {
    match icon {
        DeviceIcon::Mouse => label(locale, "display.device.mouse"),
        DeviceIcon::Keyboard => label(locale, "display.device.keyboard"),
        DeviceIcon::Headset => label(locale, "display.device.headset"),
        DeviceIcon::Gamepad => label(locale, "device.gamepad"),
        DeviceIcon::Bluetooth => label(locale, "display.device.bluetooth"),
        DeviceIcon::Receiver24g => label(locale, "display.device.receiver24g"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use battery_protocol::{DeviceIdentity, ModelSource};

    #[test]
    fn hyperx_badge_uses_confirmed_model_before_first_sample_not_vendor_alone() {
        let mut device = descriptor(
            DeviceCategory::Headset,
            ConnectionKind::Rf24,
            BatteryCapability::Present,
            Some("legacy:03f0:06be"),
        );
        assert!(is_hyperx(&device, None));
        device.id = "hid:03f0:unknown".into();
        device.identity.as_mut().unwrap().model_key = None;
        assert!(!is_hyperx(&device, None));
        assert!(is_hyperx(&device, Some(battery_protocol::Source::HyperX)));
        let identity = device.identity.as_mut().unwrap();
        identity.model_key = Some("legacy:0951:1718".into());
        identity.model_source = Some(ModelSource::ProductString);
        assert!(!is_hyperx(&device, None));
        device.identity.as_mut().unwrap().model_source = Some(ModelSource::CatalogRule);
        assert!(is_hyperx(&device, None));
    }

    #[test]
    fn turtle_beach_badge_uses_confirmed_model_before_first_sample_not_vendor_alone() {
        let mut device = descriptor(
            DeviceCategory::Headset,
            ConnectionKind::Rf24,
            BatteryCapability::Present,
            Some("legacy:10f5:2256"),
        );
        assert!(is_turtle_beach(&device, None));
        device.id = "hid:10f5:unknown".into();
        device.identity.as_mut().unwrap().model_key = None;
        assert!(!is_turtle_beach(&device, None));
        assert!(is_turtle_beach(
            &device,
            Some(battery_protocol::Source::TurtleBeach)
        ));
        let identity = device.identity.as_mut().unwrap();
        identity.model_key = Some("legacy:10f5:2256".into());
        identity.model_source = Some(ModelSource::ProductString);
        assert!(!is_turtle_beach(&device, None));
        device.identity.as_mut().unwrap().model_source = Some(ModelSource::CatalogRule);
        assert!(is_turtle_beach(&device, None));
    }

    #[test]
    fn scyrox_badge_requires_confirmed_identity_or_scyrox_vid_not_shared_vendor_alone() {
        let mut receiver = descriptor(
            DeviceCategory::Mouse,
            ConnectionKind::Rf24,
            BatteryCapability::Present,
            None,
        );
        receiver.id = "hid:395a:0002:Mouse:test".into();
        assert!(!is_scyrox(&receiver, None));
        receiver.generic_name = Some(GenericName::Receiver24g {
            brand: Some("Scyrox".into()),
            channel: Some(DeviceCategory::Mouse),
        });
        assert!(is_scyrox(&receiver, None));

        let mut wired_395a = descriptor(
            DeviceCategory::Mouse,
            ConnectionKind::Usb,
            BatteryCapability::Present,
            Some("scyrox:395a:000d"),
        );
        wired_395a.id = "hid:395a:000d:Mouse:test".into();
        assert!(is_scyrox(&wired_395a, None));

        // 仅有 395A:000F + Source::Scyrox 也属于 Scyrox 自有 VID。
        wired_395a.id = "hid:395a:000f:Mouse:test".into();
        wired_395a.identity.as_mut().unwrap().model_key = None;
        assert!(!is_scyrox(&wired_395a, None));
        assert!(is_scyrox(
            &wired_395a,
            Some(battery_protocol::Source::Scyrox)
        ));

        // ProductString 单靠字符串或共享 VID (3554 / 2FE3) 不得误判为 Scyrox。
        let mut shared = descriptor(
            DeviceCategory::Mouse,
            ConnectionKind::Usb,
            BatteryCapability::Present,
            Some("scyrox:395a:000d"),
        );
        shared.identity.as_mut().unwrap().model_source = Some(ModelSource::ProductString);
        assert!(!is_scyrox(&shared, None));
        shared.identity.as_mut().unwrap().model_source = Some(ModelSource::ProtocolIdentity);
        assert!(is_scyrox(&shared, None));

        for shared_id in ["hid:3554:f5f4:Mouse:test", "hid:2fe3:0007:Mouse:test"] {
            let mut d = descriptor(
                DeviceCategory::Mouse,
                ConnectionKind::Rf24,
                BatteryCapability::Present,
                None,
            );
            d.id = shared_id.into();
            assert!(!is_scyrox(&d, Some(battery_protocol::Source::Scyrox)));
        }
    }

    fn descriptor(
        category: DeviceCategory,
        connection: ConnectionKind,
        battery: BatteryCapability,
        model_key: Option<&str>,
    ) -> Descriptor {
        Descriptor {
            id: "device".into(),
            name: "System name".into(),
            kind: Kind::Mouse,
            legacy_ids: vec![],
            identity: Some(DeviceIdentity {
                model_key: model_key.map(str::to_owned),
                category,
                connection,
                battery,
                model_source: model_key.map(|_| ModelSource::CatalogRule),
            }),
            generic_name: None,
        }
    }

    #[test]
    fn icons_follow_identity_before_legacy_kind() {
        assert_eq!(
            device_icon(&descriptor(
                DeviceCategory::Keyboard,
                ConnectionKind::Usb,
                BatteryCapability::Unknown,
                Some("known-keyboard"),
            )),
            Some(DeviceIcon::Keyboard)
        );
        assert_eq!(
            device_icon(&descriptor(
                DeviceCategory::Unknown,
                ConnectionKind::Rf24,
                BatteryCapability::Present,
                None,
            )),
            Some(DeviceIcon::Receiver24g)
        );
        assert_eq!(
            device_icon(&descriptor(
                DeviceCategory::Headset,
                ConnectionKind::Bluetooth,
                BatteryCapability::Present,
                None,
            )),
            Some(DeviceIcon::Headset)
        );
        assert_eq!(
            device_icon(&descriptor(
                DeviceCategory::Unknown,
                ConnectionKind::Bluetooth,
                BatteryCapability::Present,
                None,
            )),
            Some(DeviceIcon::Bluetooth)
        );
        assert_eq!(
            device_icon(&descriptor(
                DeviceCategory::Mouse,
                ConnectionKind::Usb,
                BatteryCapability::Absent,
                None,
            )),
            None
        );
        let mut legacy_audio = descriptor(
            DeviceCategory::Unknown,
            ConnectionKind::Unknown,
            BatteryCapability::Unknown,
            None,
        );
        legacy_audio.identity = None;
        legacy_audio.kind = Kind::Audio;
        assert_eq!(device_icon(&legacy_audio), Some(DeviceIcon::Headset));
        assert_eq!(
            device_icon(&descriptor(
                DeviceCategory::Unknown,
                ConnectionKind::Unknown,
                BatteryCapability::Unknown,
                None,
            )),
            None
        );
    }

    #[test]
    fn only_explicit_generic_names_are_localized() {
        let mut device = descriptor(
            DeviceCategory::Mouse,
            ConnectionKind::Usb,
            BatteryCapability::Present,
            None,
        );
        assert_eq!(display_name(&device, Locale::Zh), "System name");
        device.generic_name = Some(GenericName::UnknownDevice);
        assert_eq!(display_name(&device, Locale::Zh), "鼠标");
        assert_eq!(display_name(&device, Locale::En), "Mouse");
        assert_eq!(display_name(&device, Locale::Ru), "Мышь");
        device.identity.as_mut().unwrap().connection = ConnectionKind::Rf24;
        assert_eq!(
            display_name(&device, Locale::En),
            "2.4G receiver (Mouse channel)"
        );
        device.generic_name = Some(GenericName::Receiver24g {
            brand: Some("MCHOSE".into()),
            channel: Some(DeviceCategory::Keyboard),
        });
        assert_eq!(
            display_name(&device, Locale::En),
            "MCHOSE 2.4G receiver (Keyboard channel)"
        );
        assert_eq!(
            display_name(&device, Locale::Ru),
            "MCHOSE Приёмник 2,4 ГГц (Канал клавиатуры)"
        );
    }
}
