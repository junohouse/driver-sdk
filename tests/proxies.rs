//! Checks on the contracts themselves.
//!
//! Until now nothing ran them: `build.rs` only `include_str!`s the TOML, so a malformed
//! contract compiled fine and failed later in whatever loaded it. `bundled()` is where parsing
//! and validation actually happen, so calling it is the check.

use driver_sdk::proxy::ProxyRegistry;
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
