//! Small text-formatting helpers shared across the commands.

/// Formats `count` followed by `noun`, choosing the singular or plural form of
/// the noun by appending a regular English `-s` when `count != 1`.
///
/// The tool spells out the grammatically correct form (`1 run`, `2 runs`) rather
/// than the evasive `run(s)` shorthand. Only regular `-s` plurals are supported;
/// an irregular noun (`index`/`indices`) would need its own handling.
#[must_use]
pub fn count_noun(count: usize, noun: &str) -> String {
    if count == 1 {
        format!("1 {noun}")
    } else {
        format!("{count} {noun}s")
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;

    #[test]
    fn one_is_singular() {
        assert_eq!(count_noun(1, "run"), "1 run");
    }

    #[test]
    fn zero_is_plural() {
        assert_eq!(count_noun(0, "run"), "0 runs");
    }

    #[test]
    fn many_is_plural() {
        assert_eq!(count_noun(2, "run"), "2 runs");
    }

    #[test]
    fn pluralizes_a_multi_word_noun_phrase_on_the_last_word() {
        assert_eq!(count_noun(1, "result set"), "1 result set");
        assert_eq!(count_noun(3, "result set"), "3 result sets");
    }
}
