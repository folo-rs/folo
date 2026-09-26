use crp_workspace::snapshot::SourceSnapshot;

use crate::git_fixture::Repository;
use crate::with_io_test;

#[test]
#[cfg_attr(miri, ignore = "acquires actual Git source facts")]
fn snapshot_reports_source_facts_without_release_policy() {
    with_io_test(|| {
        let fixture = Repository::new();
        fixture.write("Cargo.toml", b"[workspace]\n");
        fixture.command(&["add", "."]);
        fixture.command(&["commit", "-qm", "source"]);
        let snapshot = SourceSnapshot::discover(&fixture.path().join("Cargo.toml")).unwrap();
        assert_eq!(snapshot.root(), fixture.path().canonicalize().unwrap());
        let head = snapshot.head().unwrap();
        assert_eq!(snapshot.resolve(head.trim()).unwrap(), head);
        assert_eq!(
            snapshot.first_parent(head.trim()).unwrap().trim(),
            head.trim()
        );
        assert!(snapshot.status().unwrap().is_empty());
        assert!(snapshot.index().unwrap().starts_with(b"H "));
        snapshot.tracked("Cargo.toml".as_ref()).unwrap();
        assert!(snapshot.tracked("absent".as_ref()).is_err());
        fixture.write("Cargo.toml", b"[workspace]\nresolver = '3'\n");
        assert!(!snapshot.status().unwrap().is_empty());
    });
}
