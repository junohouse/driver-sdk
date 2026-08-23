//! Checks on the contracts themselves.
//!
//! Until now nothing ran them: `build.rs` only `include_str!`s the TOML, so a malformed
//! contract compiled fine and failed later in whatever loaded it. `bundled()` is where parsing
//! and validation actually happen, so calling it is the check.

use driver_sdk::proxy::ProxyRegistry;
use driver_sdk::proxy::resolved::CallError;
use driver_sdk::proxy::schema::ValueType;
use serde_json::{Value, json};
use std::collections::BTreeMap;

fn caps(pairs: &[(&str, Value)]) -> BTreeMap<String, Value> {
    pairs
        .iter()
        .map(|(k, v)| ((*k).to_string(), v.clone()))
        .collect()
}

fn args(pairs: &[(&str, Value)]) -> BTreeMap<String, Value> {
    caps(pairs)
}

#[test]
fn every_bundled_contract_parses_and_validates() {
    // Fails loudly listing every problem, which is the whole point of doing it here rather
    // than discovering it in a controller.
    ProxyRegistry::bundled().expect("bundled contracts must parse and validate");
}

#[test]
fn json_takes_structures_and_nothing_else() {
    assert!(ValueType::Json.accepts(&json!([{ "id": "1" }])));
    assert!(ValueType::Json.accepts(&json!({ "items": [] })));

    // The line that keeps `json` from becoming a synonym for "any".
    assert!(!ValueType::Json.accepts(&json!("a string")));
    assert!(!ValueType::Json.accepts(&json!(7)));
    assert!(!ValueType::Json.accepts(&json!(null)));
}

/// A Roku declares `has_search` and answers `search` by opening its own search screen — it
/// takes no token. Making `token` required when `browse` arrived would have broken it, and
/// nothing else in the tree would have said so.
#[test]
fn search_still_works_without_a_token() {
    let reg = ProxyRegistry::bundled().unwrap();
    let mp = reg.get("media_player").unwrap();
    let resolved = mp
        .resolve(&caps(&[("has_search", json!(true))]))
        .expect("declared capabilities resolve");

    mp.validate_call(&resolved, "search", &args(&[("query", json!("taylor swift"))]))
        .expect("a search with no token is legal");
}

/// A launcher is a source-selection action, not an assumption that every d-pad's Home key opens
/// apps. Some sets have a dedicated launcher command, and a disc player with a Home key has no
/// app launcher at all.
#[test]
fn app_launcher_is_exposed_only_when_the_driver_declares_it() {
    let reg = ProxyRegistry::bundled().unwrap();
    let mp = reg.get("media_player").unwrap();

    let plain = mp.resolve(&BTreeMap::new()).unwrap();
    assert!(!plain.supports("open_app_launcher"));

    let smart_tv = mp
        .resolve(&caps(&[("has_app_launcher", json!(true))]))
        .unwrap();
    assert!(smart_tv.supports("open_app_launcher"));
    mp.validate_call(&smart_tv, "open_app_launcher", &BTreeMap::new())
        .expect("a declared launcher takes no vendor-specific arguments");
}

#[test]
fn play_item_queue_action_is_checked_against_the_contract() {
    let reg = ProxyRegistry::bundled().unwrap();
    let mp = reg.get("media_player").unwrap();
    let resolved = mp
        .resolve(&caps(&[
            ("has_browse", json!(true)),
            ("has_queue", json!(true)),
        ]))
        .unwrap();

    mp.validate_call(
        &resolved,
        "play_item",
        &args(&[("id", json!("abc")), ("queue_action", json!("append"))]),
    )
    .expect("append is one of the four");

    // A queueless player still takes `play_item`; the parameter is optional, not gated.
    mp.validate_call(&resolved, "play_item", &args(&[("id", json!("abc"))]))
        .expect("no queue_action means play now");

    assert!(
        mp.validate_call(
            &resolved,
            "play_item",
            &args(&[("id", json!("abc")), ("queue_action", json!("someday"))]),
        )
        .is_err(),
        "a queue action outside the contract must be refused"
    );
}

/// The reason `browse_results` exists: a command cannot hand anything back, so the page of
/// results has to survive the trip out as a notification instead.
#[test]
fn browse_results_carries_a_page_of_items() {
    let reg = ProxyRegistry::bundled().unwrap();
    let mp = reg.get("media_player").unwrap();
    let resolved = mp.resolve(&caps(&[("has_browse", json!(true))])).unwrap();

    mp.validate_notification(
        &resolved,
        "browse_results",
        &args(&[
            ("token", json!("t1")),
            (
                "items",
                json!([{ "id": "fv:3", "name": "Discover Weekly",
                         "playable": true, "container": true }]),
            ),
            ("total", json!(1)),
        ]),
    )
    .expect("a page of results is a legal notification");

    // `items` is opaque, but it is still a structure — a driver sending a bare string has a
    // bug, and this is where it gets caught rather than in a control that renders nothing.
    assert!(
        mp.validate_notification(
            &resolved,
            "browse_results",
            &args(&[("items", json!("not a list"))]),
        )
        .is_err()
    );
}

/// Volume on a media player is about the hardware, not about the room.
///
/// A Sonos Amp has a volume knob whether it drives speakers or feeds a receiver, so it declares
/// one either way — the pathfinder takes the sink-most thing that can set volume, which is what
/// stops a source hijacking a room where something downstream also has it. A Roku declares none
/// and the commands are absent rather than present-and-ignored.
#[test]
fn volume_is_a_capability_of_the_box_not_of_the_room() {
    let reg = ProxyRegistry::bundled().unwrap();
    let mp = reg.get("media_player").unwrap();

    let sonos = mp
        .resolve(&caps(&[
            ("has_discrete_volume", json!(true)),
            ("has_mute", json!(true)),
        ]))
        .unwrap();
    mp.validate_call(&sonos, "set_volume", &args(&[("level", json!(35))]))
        .expect("a box with a volume knob takes set_volume");
    mp.validate_call(&sonos, "set_mute", &args(&[("mute", json!(true))]))
        .unwrap();

    let roku = mp.resolve(&BTreeMap::new()).unwrap();
    assert!(!roku.supports("set_volume"), "a streamer has no volume of its own");
    assert!(
        mp.validate_call(&roku, "set_volume", &args(&[("level", json!(35))]))
            .is_err()
    );
}

/// A player that never declared `has_browse` must not be able to emit results for it.
#[test]
fn undeclared_capabilities_do_not_leak_commands() {
    let reg = ProxyRegistry::bundled().unwrap();
    let mp = reg.get("media_player").unwrap();
    let plain = mp.resolve(&BTreeMap::new()).unwrap();

    assert!(!plain.supports("browse"));
    assert!(!plain.emits("browse_results"));
    assert!(
        mp.validate_call(&plain, "browse", &BTreeMap::new())
            .is_err()
    );
}

/// A parameter's *choices* narrow per device, the way its range already did.
///
/// `hold {what}` is one command naming the key to hold rather than eight commands. Without a
/// per-value gate that means every device advertises every key: an Apple TV over IR, which has
/// arrows and no volume at all, would be offered `volume_up` — and `validate_call` would pass
/// it to a driver whose only option is to refuse it.
///
/// This is `min_cap`/`max_cap` one type along, and for the same stated reason: never advertise
/// something the hardware silently drops.
#[test]
fn a_value_is_offered_only_when_its_capability_is_declared() {
    let registry = ProxyRegistry::bundled().unwrap();
    let player = registry.get("media_player").expect("media_player exists");
    let what = &player.commands["hold"].params["what"];

    // An IR-only box: arrows, no volume, no scan.
    let ir = caps(&[("has_hold", json!(true)), ("has_dpad", json!(true))]);
    let allowed = what.allowed(&ir);
    assert!(allowed.contains(&"up".to_string()));
    assert!(
        !allowed.contains(&"volume_up".to_string()),
        "a box with no volume must not be offered a volume ramp: {allowed:?}"
    );
    assert!(
        !allowed.contains(&"scan_forward".to_string()),
        "nor a scan key it has no command for: {allowed:?}"
    );

    // The same contract on a box that does have volume.
    let networked = caps(&[
        ("has_hold", json!(true)),
        ("has_dpad", json!(true)),
        ("has_up_down_volume", json!(true)),
    ]);
    assert!(what.allowed(&networked).contains(&"volume_up".to_string()));
}

/// And the gate is enforced, not merely advertised. A caller that ignores the offered list —
/// a rule written before a device was swapped, an assistant that guessed — is refused here
/// rather than reaching a driver.
#[test]
fn holding_a_key_the_device_lacks_is_refused_with_the_ones_that_work() {
    let registry = ProxyRegistry::bundled().unwrap();
    let player = registry.get("media_player").expect("media_player exists");

    let device = caps(&[("has_hold", json!(true)), ("has_dpad", json!(true))]);
    let resolved = player.resolve(&device).expect("resolves");

    match player.validate_call(&resolved, "hold", &args(&[("what", json!("volume_up"))])) {
        Err(CallError::NotAllowed { allowed, got, .. }) => {
            assert_eq!(got, "volume_up");
            assert!(
                allowed.contains(&"up".to_string()) && !allowed.contains(&"volume_up".to_string()),
                "the refusal should name the keys that would have worked: {allowed:?}"
            );
        }
        other => panic!("expected a refusal naming the usable keys, got {other:?}"),
    }

    // The key it does have still goes through.
    assert!(
        player
            .validate_call(&resolved, "hold", &args(&[("what", json!("left"))]))
            .is_ok()
    );
}
