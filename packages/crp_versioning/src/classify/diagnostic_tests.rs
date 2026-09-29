//! Generated classification notes, independently of Git acquisition and stderr.

use std::cell::RefCell;

use super::*;

#[test]
fn untracked_notes_are_emitted_only_for_present_paths() {
    let notes = RefCell::new(Vec::new());
    let name = "quoted\npackage";
    log_untracked(&notes, name, 0);
    assert!(notes.borrow().is_empty());

    // Exercise singular and plural counts, not any particular advisory paragraph.
    for (count, expected) in [(1, "1 untracked path"), (7, "7 untracked paths")] {
        log_untracked(&notes, name, count);
        let emitted = notes.take();
        assert_eq!(emitted.len(), 1);
        let message = emitted.first().unwrap();
        assert!(message.starts_with(&format!("{}: ", quote_path(name))));
        assert!(message.contains(expected));
        assert_eq!(message.lines().count(), 1);
    }
}

#[test]
fn status_notes_distinguish_equal_and_increased_parsed_versions() {
    // Crossing a decimal digit boundary distinguishes semver from string ordering.
    let anchor = Anchor {
        commit: "released".to_string(),
        version: Version::new(1, 9, 0),
    };
    let name = "quoted\npackage";
    let path = PathBuf::from("pkg/Cargo.toml");
    let changed = vec![ChangedItem::Inherited {
        field: "package.rust-version".to_string(),
    }];
    let classes = [
        (
            PackageClass::unchanged(name, anchor.version.clone(), anchor.clone(), path.clone()),
            false,
        ),
        (
            PackageClass::needs_increment(
                name,
                anchor.version.clone(),
                anchor.clone(),
                changed,
                path.clone(),
            ),
            false,
        ),
        (
            PackageClass::pending_release(name, Version::new(1, 10, 0), anchor, path),
            true,
        ),
    ];
    for (class, increased) in classes {
        let notes = RefCell::new(Vec::new());
        log_status(&notes, &class);
        let emitted = notes.into_inner();
        assert_eq!(emitted.len(), 1);
        let message = emitted.first().unwrap();
        assert!(message.starts_with(&format!("{}: ", quote_path(name))));
        assert!(message.contains(&format!("status {:?}", class.status())));
        assert!(message.contains(&format!("version_increased={increased}")));
        assert!(message.contains(&format!("changed_items={}", class.changed().len())));
        assert_eq!(message.lines().count(), 1);
    }
}
