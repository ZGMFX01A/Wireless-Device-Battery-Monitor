// SPDX-FileCopyrightText: 2026 ZGMFX01A
// SPDX-License-Identifier: LicenseRef-Wireless-Device-Battery-Monitor
// See LICENSE in the repository root for the full terms.

//! Match a logical keyboard to a fresh physical inventory, independently of I/O.
use crate::model::{KeyboardConnection, Prefs, State};
use battery_protocol::Reading;
use battery_protocol::hid::KeyboardPresence;

fn family(id: &str) -> Option<(&str, &str)> {
    let mut parts = id.splitn(5, ':');
    if parts.next()? != "hid" {
        return None;
    }
    let vid = parts.next()?;
    let pid = parts.next()?;
    (parts.next()? == "Keyboard" && !parts.next()?.is_empty()).then_some((vid, pid))
}

pub fn plan(
    prefs: &Prefs,
    inventory: &[KeyboardPresence],
) -> Result<Option<KeyboardConnection>, String> {
    let Some(endpoint) = prefs.keyboard_endpoint() else {
        return Ok(None);
    };
    let serial = prefs.keyboard_serial();
    if let Some(current) = inventory.iter().find(|p| p.id == endpoint) {
        if serial.is_some_and(|s| current.serial.as_deref() != Some(s)) {
            return Err(
                "键盘身份不匹配，请在添加键盘中重新选择 / Keyboard identity mismatch".into(),
            );
        }
        return Ok(None);
    }
    let Some(expected) = family(endpoint) else {
        return Err(
            "未发现原绑定键盘，请在添加键盘中重新选择 / Keyboard binding unavailable".into(),
        );
    };
    let mut matches = inventory.iter().filter(|p| {
        family(&p.id) == Some(expected) && serial.is_none_or(|s| p.serial.as_deref() == Some(s))
    });
    let Some(candidate) = matches.next() else {
        return Err("未发现匹配的键盘接收器 / No matching keyboard receiver found".into());
    };
    if matches.next().is_some() {
        return Err("发现多个匹配键盘，请在添加键盘中选择 / Multiple matching keyboards".into());
    }
    Ok(Some(KeyboardConnection {
        endpoint_id: candidate.id.clone(),
        serial: candidate.serial.clone(),
    }))
}

pub fn probe(
    prefs: &Prefs,
    inventory: Result<&[KeyboardPresence], String>,
    read: impl FnOnce(&KeyboardConnection) -> Result<Reading, String>,
) -> Result<Option<(KeyboardConnection, Reading)>, String> {
    let Some(connection) = plan(prefs, inventory?)? else {
        return Ok(None);
    };
    let sample = read(&connection)
        .map_err(|e| format!("接收器已连接，但电量暂不可读：{e} / Keyboard battery unavailable"))?;
    Ok(Some((connection, sample)))
}

/// Rebase onto the latest preferences after I/O. Never overwrite a concurrent
/// unbind/rebind, and preserve appearance/alias/BT changes made during the read.
pub fn commit(
    state: &mut State,
    before: &Prefs,
    connection: KeyboardConnection,
    save: impl FnOnce(&Prefs) -> Result<(), String>,
) -> Result<bool, String> {
    if state.prefs.keyboard != before.keyboard
        || state.prefs.keyboard_connection != before.keyboard_connection
    {
        return Ok(false);
    }
    let mut next = state.prefs.clone();
    next.reconcile_replaced_keyboard_identity(&connection);
    next.keyboard_connection = Some(connection);
    save(&next)
        .map_err(|e| format!("键盘连接配置保存失败：{e} / Keyboard preferences save failed"))?;
    state.prefs = next;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;
    const OLD: &str = "hid:0c45:fefa:Keyboard:container:799872a0d2b611f1b1e1806e6f6e6963";
    const NEW: &str = "hid:0c45:fefa:Keyboard:container:bc3b11bada8711f1b209806e6f6e6963";
    fn presence(id: &str, serial: Option<&str>) -> KeyboardPresence {
        KeyboardPresence {
            id: id.into(),
            serial: serial.map(str::to_owned),
        }
    }
    fn prefs() -> Prefs {
        serde_json::from_str(&format!(
            r#"{{"keyboard":"{OLD}","aliases":{{"{OLD}":"SK98"}}}}"#
        ))
        .unwrap()
    }
    #[test]
    fn original_container_change_recovers_old_json_and_keeps_logical_identity_after_restart() {
        let before = prefs();
        let connection = plan(&before, &[presence(NEW, None)]).unwrap().unwrap();
        let mut state = State {
            prefs: before.clone(),
            ..Default::default()
        };
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("preferences-v2.json");
        assert!(commit(&mut state, &before, connection, |p| p.save(&path)).unwrap());
        let loaded = Prefs::load(&path).unwrap();
        assert_eq!(loaded.keyboard.as_deref(), Some(OLD));
        assert_eq!(loaded.keyboard_endpoint(), Some(NEW));
        assert_eq!(loaded.aliases[OLD], "SK98");
        assert!(plan(&loaded, &[presence(NEW, None)]).unwrap().is_none());
    }
    #[test]
    fn physical_presence_blocks_replacement_even_without_a_readable_target() {
        assert!(
            plan(&prefs(), &[presence(OLD, None), presence(NEW, None)])
                .unwrap()
                .is_none()
        );
        // The inventory is captured before handshakes; an unreadable peer counts too.
        assert!(
            plan(
                &prefs(),
                &[
                    presence(NEW, None),
                    presence("hid:0c45:fefa:Keyboard:container:other", None)
                ]
            )
            .is_err()
        );
    }
    #[test]
    fn serial_match_can_select_one_of_multiple_receivers_but_conflicts_and_duplicates_cannot() {
        let mut p = prefs();
        p.keyboard_connection = Some(KeyboardConnection {
            endpoint_id: OLD.into(),
            serial: Some("abc".into()),
        });
        assert_eq!(
            plan(
                &p,
                &[
                    presence(NEW, Some("abc")),
                    presence("hid:0c45:fefa:Keyboard:container:other", Some("xyz"))
                ]
            )
            .unwrap()
            .unwrap()
            .endpoint_id,
            NEW
        );
        assert!(plan(&p, &[presence(NEW, Some("xyz"))]).is_err());
        assert!(
            plan(
                &p,
                &[
                    presence(NEW, Some("abc")),
                    presence("hid:0c45:fefa:Keyboard:container:other", Some("abc"))
                ]
            )
            .is_err()
        );
        assert!(plan(&p, &[presence(OLD, Some("xyz"))]).is_err());
    }
    #[test]
    fn no_receiver_or_wrong_model_cannot_rebind() {
        assert!(plan(&prefs(), &[]).is_err());
        assert!(
            plan(
                &prefs(),
                &[presence("hid:0c45:fefb:Keyboard:container:other", None)]
            )
            .is_err()
        );
        assert!(
            plan(
                &prefs(),
                &[presence("hid:0c45:fefa:Mouse:container:other", None)]
            )
            .is_err()
        );
    }
    #[test]
    fn failed_enumeration_and_ambiguous_presence_never_probe_or_save() {
        assert!(
            probe(&prefs(), Err("enumeration failed".into()), |_| panic!(
                "must not probe stale targets"
            ))
            .is_err()
        );
        let inventory = [
            presence(NEW, None),
            presence("hid:0c45:fefa:Keyboard:container:unreadable", None),
        ];
        assert!(
            probe(&prefs(), Ok(&inventory), |_| panic!(
                "must count unreadable peer before probing"
            ))
            .is_err()
        );
        assert!(
            probe(&prefs(), Ok(&[presence(NEW, None)]), |_| Err(
                "timeout".into()
            ))
            .is_err()
        );
    }
    #[test]
    fn save_failure_preserves_all_preferences_and_binding_changes_cancel_commit() {
        let before = prefs();
        let connection = plan(&before, &[presence(NEW, None)]).unwrap().unwrap();
        let mut state = State {
            prefs: before.clone(),
            ..Default::default()
        };
        assert!(
            commit(&mut state, &before, connection.clone(), |_| Err(
                "disk full".into()
            ))
            .is_err()
        );
        assert_eq!(state.prefs, before);
        state.prefs.keyboard = None;
        assert!(
            !commit(&mut state, &before, connection, |_| panic!(
                "must not save a stale binding"
            ))
            .unwrap()
        );
    }
    #[test]
    fn concurrent_alias_changes_are_preserved_and_new_rows_use_the_logical_key() {
        let before = prefs();
        let connection = plan(&before, &[presence(NEW, None)]).unwrap().unwrap();
        let mut state = State {
            prefs: before.clone(),
            ..Default::default()
        };
        state.prefs.aliases.insert(OLD.into(), "Renamed".into());
        assert!(
            commit(&mut state, &before, connection, |p| {
                assert_eq!(p.aliases[OLD], "Renamed");
                Ok(())
            })
            .unwrap()
        );
        let row = state.prefs.keyboard_row(battery_protocol::Descriptor {
            id: NEW.into(),
            name: "2.4G Dongle".into(),
            kind: battery_protocol::Kind::Keyboard,
            legacy_ids: vec![],
            identity: None,
            generic_name: None,
        });
        assert_eq!(row.id, OLD);
        assert_eq!(state.prefs.name(&row), "Renamed");
        assert!(state.prefs.bound(&row));
    }

    #[test]
    fn serial_less_replacement_drops_cached_exact_identity_while_serial_match_updates_endpoint() {
        use battery_protocol::{
            BatteryCapability, ConnectionKind, DeviceCategory, DeviceIdentity, GenericName,
            ModelSource,
        };
        let exact = battery_protocol::Descriptor {
            id: OLD.into(),
            name: "Exact Receiver Keyboard".into(),
            kind: battery_protocol::Kind::Keyboard,
            legacy_ids: vec![],
            identity: Some(DeviceIdentity {
                model_key: Some("exact_kb".into()),
                category: DeviceCategory::Keyboard,
                connection: ConnectionKind::Rf24,
                battery: BatteryCapability::Present,
                model_source: Some(ModelSource::CatalogRule),
            }),
            generic_name: None,
        };
        let generic_new = battery_protocol::Descriptor {
            id: NEW.into(),
            name: "2.4G Receiver".into(),
            kind: battery_protocol::Kind::Keyboard,
            legacy_ids: vec![],
            identity: Some(DeviceIdentity {
                model_key: None,
                category: DeviceCategory::Keyboard,
                connection: ConnectionKind::Rf24,
                battery: BatteryCapability::Present,
                model_source: None,
            }),
            generic_name: Some(GenericName::Receiver24g {
                brand: None,
                channel: Some(DeviceCategory::Keyboard),
            }),
        };

        // 无序列号替换同 VID/PID 接收器时，清除旧精确快照（保留别名）。
        let mut before = prefs();
        assert!(before.remember_identity(&exact));
        let connection = plan(&before, &[presence(NEW, None)]).unwrap().unwrap();
        let mut state = State {
            prefs: before.clone(),
            ..Default::default()
        };
        assert!(commit(&mut state, &before, connection, |_| Ok(())).unwrap());
        assert!(!state.prefs.bound_identity.contains_key(OLD));
        assert_eq!(state.prefs.aliases[OLD], "SK98");
        state.prefs.aliases.remove(OLD);
        let row = state.prefs.keyboard_row(generic_new.clone());
        assert_ne!(state.prefs.name(&row), "Exact Receiver Keyboard");

        // 有匹配序列号换口重连时，刷新 endpoint_id 并保持精确名称与图标。
        let mut with_serial = prefs();
        with_serial.aliases.clear();
        with_serial.keyboard_connection = Some(KeyboardConnection {
            endpoint_id: OLD.into(),
            serial: Some("SN-SAME".into()),
        });
        assert!(with_serial.remember_identity(&exact));
        let connection = plan(&with_serial, &[presence(NEW, Some("SN-SAME"))])
            .unwrap()
            .unwrap();
        let mut state = State {
            prefs: with_serial.clone(),
            ..Default::default()
        };
        assert!(commit(&mut state, &with_serial, connection, |_| Ok(())).unwrap());
        let row = state.prefs.keyboard_row(generic_new);
        assert_eq!(state.prefs.name(&row), "Exact Receiver Keyboard");
    }
}
