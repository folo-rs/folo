use azure_core::Uuid;

/// Generates an isolated container name for Azure and Azurite tests.
pub fn unique_test_container() -> String {
    // Random UUIDs isolate nextest processes and concurrent jobs, including jobs on different
    // machines. Neither wall-clock precision nor a process-local counter provides that isolation.
    unique_test_container_with(Uuid::new_v4)
}

fn unique_test_container_with(next_uuid: impl FnOnce() -> Uuid) -> String {
    // Keep the prefix recognized by infra/azure-bench-history-test/cleanup-containers.ps1.
    format!("bh-it-{}", next_uuid())
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use std::ops::RangeInclusive;

    use super::*;

    #[test]
    fn independent_identities_produce_distinct_valid_container_names() {
        // Azure Blob container naming limits.
        const CONTAINER_NAME_LENGTH: RangeInclusive<usize> = 3..=63;

        // Boundary identities ensure the entire UUID is retained, not just a truncated suffix.
        // Each call has independent local state and needs neither a clock nor a counter.
        let identities = [
            Uuid::from_u128(0),
            Uuid::from_u128(1),
            Uuid::from_u128(1 << 127),
            Uuid::from_u128(u128::MAX),
        ];
        let names = identities.map(|identity| {
            let name = unique_test_container_with(|| identity);
            assert!(CONTAINER_NAME_LENGTH.contains(&name.len()));
            assert!(
                name.bytes()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == b'-')
            );
            assert!(!name.contains("--"));
            assert!(name.starts_with("bh-it-"));
            assert!(name.ends_with(|c: char| c.is_ascii_alphanumeric()));
            assert_eq!(
                Uuid::parse_str(name.strip_prefix("bh-it-").unwrap()).unwrap(),
                identity
            );
            name
        });
        let mut distinct = names.to_vec();
        distinct.sort();
        distinct.dedup();
        assert_eq!(distinct.len(), identities.len());
    }

    #[test]
    #[cfg_attr(miri, ignore = "uses operating-system randomness")]
    fn generated_containers_use_fresh_random_uuids() {
        // UUID version identifying random rather than time-based identities.
        const RANDOM_UUID_VERSION: usize = 4;

        let names = [unique_test_container(), unique_test_container()];
        for name in &names {
            let identity = Uuid::parse_str(name.strip_prefix("bh-it-").unwrap()).unwrap();
            assert_eq!(identity.get_version_num(), RANDOM_UUID_VERSION);
        }
        assert_ne!(names.first(), names.last());
    }
}
