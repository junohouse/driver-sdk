//! Panes a driver declares, and what is refused before one reaches a house.
//!
//! A declared pane is drawn by the configurator's own components, so the driver author never
//! sees it fail: a `from` that names nothing draws an em dash, which is exactly what a radio
//! that has not reported yet draws. The two have to be told apart somewhere, and the only place
//! anybody is looking at the driver they just wrote is the install.

use driver_sdk::manifest::{BlockDecl, Manifest};
use driver_sdk::proxy::ProxyRegistry;

const HEAD: &str = r#"
[driver]
id = "test.panes"
name = "Panes"
manufacturer = "Test"
version = "1.0.0"
runtime = "declarative"

[[proxy]]
id = 1
type = "switch"
primary = true
"#;

fn problems(extra: &str) -> Vec<String> {
    let manifest = Manifest::parse(&format!("{HEAD}{extra}")).expect("parses");
    manifest.validate(&ProxyRegistry::bundled().unwrap())
}

#[test]
fn a_pane_of_blocks_is_read_as_written() {
    let manifest = Manifest::parse(&format!("{HEAD}{}", r#"
[[action]]
name = "heal"
label = "Heal the mesh"

[[tab]]
id = "status"
title = "Status"
on = "adapter"

[[tab.block]]
kind = "stats"
title = "Right now"
[[tab.block.field]]
from = "detail.counts.total"
label = "Devices"
[[tab.block.field]]
from = "state.on"
label = "Powered"
format = "bool"

[[tab.block]]
kind = "table"
from = "detail.devices"
[[tab.block.column]]
path = "name"
label = "Device"

[[tab.block]]
kind = "actions"
action = ["heal"]

[[tab.block]]
kind = "text"
body = "What the buttons do."
"#)).expect("parses");

    let tab = &manifest.tab[0];
    assert_eq!(tab.id, "status");
    assert_eq!(tab.block.len(), 4);
    // The block kinds survive as themselves rather than as a bag of optional fields — a table
    // with no `from` cannot be constructed, which is the point of the tagged shape.
    assert!(matches!(&tab.block[0], BlockDecl::Stats { field, .. } if field.len() == 2));
    assert!(matches!(&tab.block[1], BlockDecl::Table { from, .. } if from == "detail.devices"));
    assert!(matches!(&tab.block[2], BlockDecl::Actions { action, .. } if action == &["heal"]));
    assert!(matches!(&tab.block[3], BlockDecl::Text { .. }));
    assert!(problems("").is_empty());
}

#[test]
fn a_pane_with_no_blocks_is_the_drivers_own_page() {
    // The escape hatch, and it has to stay silent: a mesh map and three thousand converter rows
    // are not something to express in a manifest, and a driver saying so is not incomplete.
    let manifest = Manifest::parse(&format!("{HEAD}{}", r#"
[[tab]]
id = "mesh"
title = "Mesh"
on = "coordinator"
"#)).expect("parses");
    assert!(manifest.tab[0].block.is_empty());
    assert!(manifest.validate(&ProxyRegistry::bundled().unwrap()).is_empty());
}

#[test]
fn a_source_nothing_can_read_is_refused_at_install() {
    // `sate.level` draws an em dash for ever, and an em dash is what a device that has not
    // reported draws. Nothing downstream can tell the difference, so this is the only chance.
    let errs = problems(r#"
[[tab]]
id = "status"
title = "Status"
[[tab.block]]
kind = "stats"
[[tab.block.field]]
from = "sate.level"
label = "Level"
"#);
    assert_eq!(errs.len(), 1, "{errs:?}");
    assert!(errs[0].contains("sate"), "{errs:?}");
    assert!(errs[0].contains("state, property or detail"), "{errs:?}");
}

#[test]
fn a_source_with_no_key_is_refused_too() {
    let errs = problems(r#"
[[tab]]
id = "status"
title = "Status"
[[tab.block]]
kind = "stats"
[[tab.block.field]]
from = "state"
label = "Something"
"#);
    assert_eq!(errs.len(), 1, "{errs:?}");
    assert!(errs[0].contains("names no key"), "{errs:?}");
}

#[test]
fn a_button_for_an_action_the_driver_does_not_have_is_refused() {
    // The rename that would otherwise leave a button drawn against nothing. It is a button that
    // reports "no such action" when pressed, months later, to somebody who did not write it.
    let errs = problems(r#"
[[tab]]
id = "status"
title = "Status"
[[tab.block]]
kind = "actions"
action = ["heal"]
"#);
    assert_eq!(errs.len(), 1, "{errs:?}");
    assert!(errs[0].contains("does not declare"), "{errs:?}");
}

#[test]
fn two_panes_cannot_share_an_id() {
    // The id is what the configurator hands back to say which pane to draw. Two of them is a
    // tab strip where one of the two is unreachable and nothing says which.
    let errs = problems(r#"
[[tab]]
id = "status"
title = "Status"
[[tab]]
id = "status"
title = "Also status"
"#);
    assert!(errs.iter().any(|e| e.contains("duplicate tab")), "{errs:?}");
}

#[test]
fn an_empty_table_or_an_empty_paragraph_is_a_mistake() {
    let errs = problems(r#"
[[tab]]
id = "status"
title = "Status"
[[tab.block]]
kind = "table"
from = "detail.devices"
column = []
[[tab.block]]
kind = "text"
body = "   "
"#);
    assert!(errs.iter().any(|e| e.contains("no columns")), "{errs:?}");
    assert!(errs.iter().any(|e| e.contains("nothing in it")), "{errs:?}");
}
