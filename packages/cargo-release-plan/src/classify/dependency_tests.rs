//! Historical dependency paths preserve nested workspace membership boundaries.

use super::*;
use crate::git::testing::Repository;

#[test]
#[cfg_attr(miri, ignore = "reads historical manifests from a real Git repository")]
fn historical_paths_distinguish_local_inherited_and_outside_members() {
    let fixture = Repository::new();
    let root = "[workspace]\n\
        members = ['packages/member']\n\
        [workspace.dependencies]\n\
        inherited = { path = 'inherited' }\n\
        root = { path = '.' }\n\
        parent = { path = '..' }\n\
        outside = { path = '../outside' }\n";
    fixture.write("nested/Cargo.toml", root.as_bytes());
    fixture.write(
        "nested/packages/member/Cargo.toml",
        b"[package]\nname = 'member'\nversion = '1.2.3'\n\
          [dependencies]\nlocal = { path = '../local' }\n\
          root_local = { path = '../..' }\n\
          parent_local = { path = '../../..' }\n\
          outside_local = { path = '../../../outside' }\n\
          beyond_repository = { path = '../../../../missing' }\n\
          inherited.workspace = true\nroot.workspace = true\n\
          parent.workspace = true\noutside.workspace = true\n",
    );
    for path in ["nested/packages/local", "nested/inherited", "outside"] {
        fixture.write(
            &format!("{path}/Cargo.toml"),
            b"[package]\nname = 'helper'\nversion = '1.2.3'\n",
        );
    }
    fixture.command(&["add", "nested", "outside"]);
    fixture.command(&["commit", "--quiet", "-m", "dependency paths"]);
    let commit = fixture.command(&["rev-parse", "HEAD"]);
    let git = fixture.repo();
    let root = parse_document(Path::new("nested/Cargo.toml"), root).unwrap();
    let paths = [
        ("", "nested/Cargo.toml"),
        ("packages/member", "nested/packages/member/Cargo.toml"),
        ("packages/local", "nested/packages/local/Cargo.toml"),
        ("inherited", "nested/inherited/Cargo.toml"),
        ("../outside", "outside/Cargo.toml"),
    ]
    .map(|(directory, path)| (directory.to_string(), path.to_string()))
    .into();
    let mut source = GitManifestSource {
        git: &git,
        commit: commit.trim(),
        workspace_prefix: "nested/",
        workspace: WorkspaceInherit::from_root(&root),
        paths,
        parsed: BTreeMap::new(),
    };

    assert_eq!(
        source.candidate_dirs(),
        [
            "",
            "../outside",
            "inherited",
            "packages/local",
            "packages/member"
        ]
    );
    let edges = source.path_edges("packages/member").unwrap();
    assert_eq!(
        edges.into_iter().collect::<BTreeSet<_>>(),
        ["", "inherited", "packages/local"]
            .map(str::to_string)
            .into()
    );
    assert!(source.path_edges("").unwrap().is_empty());
    assert!(source.path_edges("absent").unwrap().is_empty());

    let members = parse_workspace_members(
        &root.to_string(),
        Path::new("nested/Cargo.toml"),
        PathCase::Sensitive,
    )
    .unwrap();
    assert_eq!(
        resolve_members(&mut source, &members).unwrap(),
        ["", "inherited", "packages/local", "packages/member"]
            .map(str::to_string)
            .into()
    );
}
