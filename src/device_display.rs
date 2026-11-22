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

pub fn device_icon(device: &Descriptor) -> Option<DeviceIcon> {
    let Some(identity) = &device.identity else {
        return Some(match device.kind {
            Kind::Mouse => DeviceIcon::Mouse,
            Kind::Keyboard => DeviceIcon::Keyboard,
            Kind::Bluetooth => DeviceIcon::Bluetooth,
            Kind::Audio => DeviceIcon::Headset,
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
        Some(DeviceCategory::Headset | DeviceCategory::Unknown) | None => None,
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
