//! Acceptance test 1 (SPEC §11): the default key map equals the one observed at
//! runtime in Commander One's defaults (`docs/evidence/default-keymap.observed.json`).

use std::collections::HashMap;

use fc_core::keymap::{Action, Chord, Keymap};

#[derive(serde::Deserialize)]
struct ObservedRow {
    action: String,
    raw: u64,
}

fn observed() -> Vec<ObservedRow> {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../docs/evidence/default-keymap.observed.json");
    serde_json::from_str(&std::fs::read_to_string(path).expect("evidence file")).expect("evidence json")
}

#[test]
fn every_observed_binding_decodes_to_the_default() {
    let rows = observed();
    assert_eq!(rows.len(), 87);
    let defaults = Keymap::commander_one_defaults();
    for row in &rows {
        let chord = Chord::from_commander_one_raw(row.raw).unwrap_or_else(|| panic!("undecodable raw {:#x}", row.raw));
        let action = Action::from_id(&row.action).unwrap_or_else(|| panic!("unknown action id {}", row.action));
        assert_eq!(defaults.action_for(&chord), Some(action), "{chord} ({:#x}) should run {}", row.raw, row.action);
    }
}

#[test]
fn importing_commander_one_storage_reproduces_defaults_in_order() {
    let rows = observed();
    let stored = serde_json::json!({
        "keyStorage": rows.iter().map(|r| r.raw).collect::<Vec<_>>(),
        "pairStorage": rows.iter().map(|r| (r.raw.to_string(), r.action.clone())).collect::<HashMap<_, _>>(),
    });
    let (imported, skipped) = Keymap::from_commander_one_json(&stored.to_string()).unwrap();
    assert!(skipped.is_empty(), "skipped: {skipped:?}");
    assert_eq!(imported, Keymap::commander_one_defaults());
}

#[test]
fn spot_checks_from_the_spec() {
    let cases = [
        (0x820064, "shift-f8", Action::DeleteForce),
        (0x800063, "f3", Action::View),
        (0x0a000e, "alt-shift-e", Action::SwapPanels),
        (0x300045, "cmd-num+", Action::SelectAll),
        (0x00000a, "§", Action::SelectItem),
        (0x10007e, "cmd-up", Action::GoTop),
        (0x8c0062, "ctrl-alt-f7", Action::SearchSpotlight),
    ];
    let km = Keymap::commander_one_defaults();
    for (raw, text, action) in cases {
        let chord = Chord::from_commander_one_raw(raw).unwrap();
        assert_eq!(chord, Chord::parse(text).unwrap());
        assert_eq!(km.action_for(&chord), Some(action));
    }
}
