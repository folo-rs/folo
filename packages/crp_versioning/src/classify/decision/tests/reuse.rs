//! Successful decision reuse and error admission.

use std::io;

use crp_diag::{Discard, Verbose};
use crp_workspace::cache::Cache;
use semver::Version;

use super::fixture::{compute, inputs, package};
use crate::VersionRegressionError;
use crate::classify::decision::DecisionCache;

#[test]
fn equal_inputs_skip_actual_computation_in_memory_and_in_a_new_invocation() {
    let input = inputs();
    let quiet = Verbose::new(false, &Discard);
    let mut cache = DecisionCache::default();
    let first = cache
        .get_with(
            input.key(quiet).unwrap(),
            quiet,
            |_, call| call(),
            || compute(&input),
        )
        .unwrap();
    let memory = cache
        .get_with(
            input.key(quiet).unwrap(),
            quiet,
            |_, _| panic!(),
            || panic!(),
        )
        .unwrap();
    let persisted = serde_json::to_vec(&first).unwrap();
    let independent = DecisionCache::default()
        .get_with(
            input.key(quiet).unwrap(),
            quiet,
            |_, _| Ok(serde_json::from_slice(&persisted).unwrap()),
            || panic!(),
        )
        .unwrap();
    assert_eq!(
        serde_json::to_value(first).unwrap(),
        serde_json::to_value(memory).unwrap()
    );
    assert_eq!(serde_json::to_vec(&independent).unwrap(), persisted);
}

#[test]
fn errors_and_disabled_storage_never_populate_successful_memory() {
    let input = inputs();
    let quiet = Verbose::new(false, &Discard);
    let mut cache = DecisionCache::default();
    let error = cache
        .get_with(
            input.key(quiet).unwrap(),
            quiet,
            |_, call| call(),
            || Err(io::Error::other("decision failure").into()),
        )
        .unwrap_err();
    assert!(error.find_source::<io::Error>().is_some());
    assert!(cache.last.is_none());
    cache
        .get(&input, &Cache::default(), quiet, || compute(&input))
        .unwrap();
    assert!(cache.last.is_none());
    let mut regression = input.clone();
    package(&mut regression).version = Version::new(0, 1, 0);
    let error = cache
        .get_with(
            regression.key(quiet).unwrap(),
            quiet,
            |_, call| call(),
            || compute(&regression),
        )
        .unwrap_err();
    assert!(error.find_source::<VersionRegressionError>().is_some());
    assert!(cache.last.is_none());
    cache
        .get_with(
            input.key(quiet).unwrap(),
            quiet,
            |_, call| call(),
            || compute(&input),
        )
        .unwrap();
    assert!(cache.last.is_some());
}

#[test]
fn immutable_rendering_errors_do_not_publish_decisions() {
    let input = inputs();
    let quiet = Verbose::new(false, &Discard);
    let mut cache = DecisionCache::default();
    let error = cache
        .get_with(
            input.key(quiet).unwrap(),
            quiet,
            |_, compute| compute(),
            || {
                input.compute(
                    |ids| Ok(vec![0; ids.len()]),
                    |_| Err(io::Error::other("object unavailable").into()),
                )
            },
        )
        .unwrap_err();
    assert!(error.find_source::<io::Error>().is_some());
    assert!(cache.last.is_none());
}
