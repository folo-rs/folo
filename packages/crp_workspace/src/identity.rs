/// Recognizes a complete Git object identity without selecting a revision.
pub fn immutable_commit(value: &str) -> bool {
    // Git supports these full object identities; abbreviated revisions are not transport identities.
    matches!(value.len(), 40 | 64)
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;

    #[test]
    fn requires_complete_lowercase_object_ids() {
        for length in [40, 64] {
            assert!(immutable_commit(&"a".repeat(length)));
            assert!(immutable_commit(&"0".repeat(length)));
            assert!(!immutable_commit(&"A".repeat(length)));
            assert!(!immutable_commit(&"g".repeat(length)));
        }
        for length in [0, 7, 39, 41, 63, 65] {
            assert!(!immutable_commit(&"a".repeat(length)));
        }
    }
}
