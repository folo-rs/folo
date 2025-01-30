use itertools::{FoldWhile, Itertools};
use nonempty::NonEmpty;

use std::collections::VecDeque;

use crate::ProcessorId;

// https://github.com/cloudhead/nonempty/issues/68
extern crate alloc;

// TODO: Package this better in some more useful and generalized way.

/// Parses a cpulist string in "0,1,2-4,5-9:2,6-10:2" format, typically used in Linux tooling.
///
/// The string is a comma-separated list of ranges, where each range is either:
/// * a single number
/// * a range
/// * a range with a stride (step size) operator
///
/// Whitespace is not allowed in the input.
///
/// Returns the numeric indexes in ascending sorted order. Removes duplicates if any exist.
pub fn parse(cpulist: &str) -> NonEmpty<ProcessorId> {
    NonEmpty::from_vec(
        cpulist
            .split(',')
            .flat_map(|part| {
                if let Some((range_start, range_end_inc)) = part.split_once('-') {
                    // This is a range, with optional stride.
                    let range_start = range_start
                        .parse::<ProcessorId>()
                        .expect("failed to parse cpulist range start as integer");

                    // If no stride is specified, we just default to 1 and pretend it was specified.
                    let (range_end_inc, stride) =
                        if let Some((range_end_inc, stride)) = range_end_inc.split_once(':') {
                            (
                                range_end_inc
                                    .parse::<ProcessorId>()
                                    .expect("failed to parse cpulist range end as integer"),
                                stride
                                    .parse::<ProcessorId>()
                                    .expect("failed to parse cpulist range stride as integer"),
                            )
                        } else {
                            (
                                range_end_inc
                                    .parse::<ProcessorId>()
                                    .expect("failed to parse cpulist range end as integer"),
                                1,
                            )
                        };

                    assert_ne!(stride, 0, "cpulist range stride must not be zero");
                    assert!(
                        range_start <= range_end_inc,
                        "cpulist range start must be <= end"
                    );

                    (range_start..=range_end_inc)
                        .step_by(stride as usize)
                        .collect_vec()
                } else {
                    // This is a plain number.
                    vec![part
                        .parse::<ProcessorId>()
                        .expect("failed to parse cpulist single entry as integer")]
                }
            })
            .sorted()
            .dedup()
            .collect_vec(),
    )
    .expect("cpulist cannot be empty")
}

/// Generates a cpulist that can be parsed by `parse()`.
pub fn emit(items: NonEmpty<ProcessorId>) -> String {
    // Sorted remaining processor IDs that we have not yet grouped.

    let mut remaining = items
        .iter()
        .unique()
        .sorted_unstable()
        .collect::<VecDeque<_>>();

    // We want to coalesce consecutive numbers into groups (ranges).
    // Each group is (start ID, len).
    let mut groups: Vec<(ProcessorId, u32)> = Vec::new();

    while !remaining.is_empty() {
        let group = remaining.iter().fold_while(
            None,
            |acc: Option<(ProcessorId, u32)>, p: &&ProcessorId| {
                if let Some((start, len)) = acc {
                    if start + len == **p {
                        // This processor ID is part of the current group.
                        FoldWhile::Continue(Some((start, len + 1)))
                    } else {
                        // This processor ID is not part of the current group.
                        FoldWhile::Done(Some((start, len)))
                    }
                } else {
                    // Start a new group.
                    FoldWhile::Continue(Some((**p, 1)))
                }
            },
        );

        let (start, len) = group
            .into_inner()
            .expect("must be Some if we still have remaining items");

        groups.push((start, len));

        for _ in 0..len {
            remaining.pop_front();
        }
    }

    let mut result = String::new();

    for (start, len) in groups {
        if !result.is_empty() {
            result.push(',');
        }

        if len == 1 {
            result.push_str(&start.to_string());
        } else if len == 2 {
            result.push_str(&format!("{},{}", start, start + 1));
        } else {
            result.push_str(&format!("{}-{}", start, start + len - 1));
        }
    }

    result
}

#[cfg(test)]
mod tests {
    use super::*;

    use nonempty::nonempty;

    #[test]
    fn parse_smoke_test() {
        assert_eq!(parse("555"), nonempty![555]);

        assert_eq!(parse("0,1,2,3"), nonempty![0, 1, 2, 3]);

        assert_eq!(parse("2,3,1"), nonempty![1, 2, 3]);

        assert_eq!(parse("0-5,1-6"), nonempty![0, 1, 2, 3, 4, 5, 6]);

        assert_eq!(parse("0-0:5"), nonempty![0]);

        assert_eq!(parse("0-0,1-1,3-3"), nonempty![0, 1, 3]);
        assert_eq!(parse("0-10:3,5-15:3"), nonempty![0, 3, 5, 6, 8, 9, 11, 14]);

        assert_eq!(parse("0-10:999999"), nonempty![0]);

        assert_eq!(
            parse("0,1,2-4,5-9:2,6-10:2"),
            nonempty![0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10]
        );
    }

    #[test]
    fn emit_smoke_test() {
        assert_eq!(emit(nonempty![555]), "555");

        assert_eq!(emit(nonempty![555, 666]), "555,666");

        assert_eq!(emit(nonempty![0, 1, 2, 3]), "0-3");

        assert_eq!(
            emit(nonempty![0, 1, 2, 3, 6, 7, 8, 11, 12, 13]),
            "0-3,6-8,11-13"
        );

        assert_eq!(emit(nonempty![1, 2, 3]), "1-3");

        assert_eq!(emit(nonempty![0, 1, 2, 3, 4, 5, 6]), "0-6");

        assert_eq!(emit(nonempty![0]), "0");

        assert_eq!(emit(nonempty![0, 1, 3]), "0,1,3");

        assert_eq!(
            emit(nonempty![0, 3, 5, 6, 8, 9, 11, 14]),
            "0,3,5,6,8,9,11,14"
        );

        assert_eq!(emit(nonempty![0]), "0");

        assert_eq!(emit(nonempty![0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10]), "0-10");
    }

    #[test]
    #[should_panic]
    fn empty_panics() {
        parse("");
    }

    #[test]
    #[should_panic]
    fn zero_stride_panics() {
        parse("1-22:0");
    }

    #[test]
    #[should_panic]
    fn range_direction_fail_panics() {
        parse("2-1");
    }
}
