use crp_diag::Verbose;
use semver::Version;
use serde_json::json;

use crate::plan::{IncrementLevel, increment_version, resolve_plan};
use crate::propose::tests::{
    assert_invariants, depends, entries, generate, helper, package, report,
};

#[test]
fn semantic_member_entries_keep_their_names_and_merge_only_in_the_resolver() {
    let report = report(
        vec![
            package("nm", "1.2.0", Some("1.0.0")),
            package("nm_impl", "1.2.0", Some("1.2.0")),
        ],
        vec![],
        &[&["nm", "nm_impl"]],
    );
    let plan = generate(&report, &[("nm", "nonbreaking"), ("nm_impl", "patch")]).unwrap();
    assert_eq!(
        entries(&plan),
        json!([{"name": "nm_impl", "level": "patch"}])
    );
    let plan = generate(&report, &[("nm", "breaking"), ("nm_impl", "patch")]).unwrap();
    assert_eq!(plan.increments.len(), 2);
    assert_invariants(&report, &plan);
}

#[test]
fn anchorless_dependents_and_helpers_do_not_invent_decisions() {
    let report = report(
        vec![
            package("lib", "2.0.0", Some("1.0.0")),
            depends(package("new", "0.1.0", None), "lib", true),
            depends(package("other", "1.0.0", Some("1.0.0")), "helper", true),
        ],
        vec![helper("helper", "1.0.0")],
        &[],
    );
    assert!(generate(&report, &[]).unwrap().increments.is_empty());
}

#[test]
fn ordered_output_sorts_unsorted_decisions() {
    assert_ordered_output(false);
}

#[test]
fn ordered_output_is_unchanged_by_reversed_report_and_decisions() {
    assert_ordered_output(true);
}

fn assert_ordered_output(reverse: bool) {
    let mut report = report(
        vec![
            package("alpha", "1.0.0", Some("1.0.0")),
            package("zeta", "1.0.0", Some("1.0.0")),
            package("laggard", "1.0.0", Some("1.0.0")),
            package("leader", "1.1.0", Some("1.1.0")),
        ],
        vec![],
        &[&["laggard", "leader"]],
    );
    let mut changes = [("zeta", "patch"), ("alpha", "patch")];
    if reverse {
        report.packages.reverse();
        changes.reverse();
    }
    let plan = generate(&report, &changes).unwrap();
    assert_eq!(
        entries(&plan),
        json!([
            {"name": "alpha", "level": "patch"},
            {"name": "laggard", "version": "1.1.0"},
            {"name": "zeta", "level": "patch"}
        ])
    );
    assert_invariants(&report, &plan);
}

#[test]
fn empty_and_consistent_reports_generate_empty_proposals() {
    for report in [
        report(vec![], vec![], &[]),
        report(
            vec![
                package("a", "1.0.0", Some("1.0.0")),
                package("b", "1.0.0", Some("1.0.0")),
            ],
            vec![],
            &[&["a", "b"]],
        ),
    ] {
        assert!(generate(&report, &[]).unwrap().increments.is_empty());
    }
}

#[test]
fn mechanical_levels_use_the_original_group_base_only_once() {
    let report = report(
        vec![
            package("a", "1.0.0", Some("1.0.0")),
            package("b", "2.3.4", Some("2.3.4")),
        ],
        vec![],
        &[&["a", "b"]],
    );
    let plan = generate(&report, &[("a", "breaking"), ("b", "nonbreaking")]).unwrap();
    let resolved = resolve_plan(
        &plan,
        &report.version_groups(),
        &report.version_targets(),
        Verbose::new(false, &crp_diag::Discard),
    )
    .unwrap();
    let expected = increment_version(&Version::new(2, 3, 4), IncrementLevel::Major).unwrap();
    assert_eq!(resolved.packages.get("a"), Some(&expected));
    assert_eq!(resolved.packages.get("b"), Some(&expected));
}
