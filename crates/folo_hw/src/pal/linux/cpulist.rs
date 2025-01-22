use itertools::Itertools;
use nonempty::NonEmpty;

use crate::pal::ProcessorGlobalIndex;

// https://github.com/cloudhead/nonempty/issues/68
extern crate alloc;

/// Parses a cpulist string in "0,1,2-4,5-9:2,6-10:2" format.
///
/// The string is a comma-separated list of ranges, where each range is either:
/// * a single number
/// * a range
/// * a range with a stride (step size) operator
///
/// Whitespace is not allowed in the input.
///
/// Returns the numeric indexes in ascending sorted order. Removes duplicates if any exist.
pub(crate) fn parse(cpulist: &str) -> NonEmpty<ProcessorGlobalIndex> {
    NonEmpty::from_vec(
        cpulist
            .split(',')
            .flat_map(|part| {
                if let Some((range_start, range_end_inc)) = part.split_once('-') {
                    // This is a range, with optional stride.
                    let range_start = range_start
                        .parse::<ProcessorGlobalIndex>()
                        .expect("failed to parse cpulist range start as integer");

                    // If no stride is specified, we just default to 1 and pretend it was specified.
                    let (range_end_inc, stride) =
                        if let Some((range_end_inc, stride)) = range_end_inc.split_once(':') {
                            (
                                range_end_inc
                                    .parse::<ProcessorGlobalIndex>()
                                    .expect("failed to parse cpulist range end as integer"),
                                stride
                                    .parse::<ProcessorGlobalIndex>()
                                    .expect("failed to parse cpulist range stride as integer"),
                            )
                        } else {
                            (
                                range_end_inc
                                    .parse::<ProcessorGlobalIndex>()
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
                        .parse::<ProcessorGlobalIndex>()
                        .expect("failed to parse cpulist single entry as integer")]
                }
            })
            .sorted()
            .dedup()
            .collect_vec(),
    )
    .expect("cpulist cannot be empty")
}

#[cfg(test)]
mod tests {
    use super::*;

    use nonempty::nonempty;

    #[test]
    fn smoke_test() {
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
