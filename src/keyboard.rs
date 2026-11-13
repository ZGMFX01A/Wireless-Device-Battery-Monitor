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
    let serial = prefs
        .keyboard_connection
        .as_ref()
        .and_then(|c| c.serial.as_deref())
        .or_else(|| {
            endpoint
                .split_once(":serial:")
                .map(|(_, serial)| serial)
                .filter(|serial| !serial.is_empty() && !serial.chars().all(|c| c == '0'))
        });
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
        });
        assert_eq!(row.id, OLD);
        assert_eq!(state.prefs.name(&row), "Renamed");
        assert!(state.prefs.bound(&row));
    }
}
