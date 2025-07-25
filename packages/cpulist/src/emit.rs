use std::collections::VecDeque;
use std::fmt::Write;
use std::num::NonZero;

use itertools::{FoldWhile, Itertools};
use new_zealand::nz;

use crate::Item;

/// Generates a [cpulist][crate] in a format that can be parsed by [`parse()`][crate::parse].
///
/// Empty input is valid and returns an empty string.
///
/// The exact emitted representation is unspecified and may change across versions of this package.
/// All we promise is that it is a recognizable cpulist and can be parsed by this package.
///
/// See [package-level documentation][crate] for more details.
pub fn emit<I>(items: I) -> String
where
    I: IntoIterator,
    I::Item: Into<Item>,
{
    // We group consecutive items to generate shorter output strings.
    // Sorted remaining items that we have not yet grouped.
    let mut remaining = items
        .into_iter()
        .map(Into::into)
        .unique()
        .sorted_unstable()
        .collect::<VecDeque<_>>();

    // We want to coalesce consecutive numbers into groups (ranges).
    // Each group is (start ID, len).
    let mut groups: Vec<(Item, NonZero<Item>)> = Vec::new();

    while !remaining.is_empty() {
        let group = remaining
            .iter()
            .fold_while(None, |acc: Option<(Item, NonZero<Item>)>, p: &Item| {
                if let Some((start, len)) = acc {
                    let expected_next_p = start.checked_add(len.get())
                        .expect("overflow impossible unless we iterate far beyond any realistic processor ID range");

                    if expected_next_p == *p {
                        let new_len = len.checked_add(1)
                            .expect("overflow impossible unless we iterate far beyond any realistic processor ID range");

                        // This item is part of the current group.
                        FoldWhile::Continue(Some((start, new_len)))
                    } else {
                        // This item is not part of the current group.
                        FoldWhile::Done(Some((start, len)))
                    }
                } else {
                    // Start a new group.
                    FoldWhile::Continue(Some((*p, nz!(1))))
                }
            });

        let (start, len) = group
            .into_inner()
            .expect("this must be Some if we still have remaining items");

        groups.push((start, len));

        for _ in 0..len.get() {
            remaining.pop_front();
        }
    }

    let mut result = String::new();

    for (start, len) in groups {
        if !result.is_empty() {
            result.push(',');
        }

        let len = len.get();

        if len == 1 {
            // A range of one item - just emit the item.
            result.push_str(&start.to_string());
        } else if len == 2 {
            // If the range only has two items, we emit them separately.
            let second_processor_id = start.checked_add(1).expect(
                "overflow impossible unless we far exceed any realistic processor ID range",
            );

            write!(result, "{start},{second_processor_id}").unwrap();
        } else {
            let last_processor_id = start
                .checked_add(len)
                .expect("overflow impossible unless we far exceed any realistic processor ID range")
                .checked_sub(1)
                .expect("cannot underflow because len is NonZero");

            write!(result, "{start}-{last_processor_id}").unwrap();
        }
    }

    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn emit_smoke_test() {
        assert_eq!(emit::<[u32; 0]>([]), "");

        assert_eq!(emit([555_u32]), "555");

        assert_eq!(emit([555_u32, 666_u32]), "555,666");

        assert_eq!(emit([0_u32, 1_u32, 2_u32, 3_u32]), "0-3");

        assert_eq!(
            emit([
                0_u32, 1_u32, 2_u32, 3_u32, 6_u32, 7_u32, 8_u32, 11_u32, 12_u32, 13_u32
            ]),
            "0-3,6-8,11-13"
        );

        assert_eq!(emit([1_u32, 2_u32, 3_u32]), "1-3");

        assert_eq!(
            emit([0_u32, 1_u32, 2_u32, 3_u32, 4_u32, 5_u32, 6_u32]),
            "0-6"
        );

        assert_eq!(emit([0_u32]), "0");

        assert_eq!(emit([0_u32, 1_u32, 3_u32]), "0,1,3");

        assert_eq!(
            emit([0_u32, 3_u32, 5_u32, 6_u32, 8_u32, 9_u32, 11_u32, 14_u32]),
            "0,3,5,6,8,9,11,14"
        );

        assert_eq!(emit([0_u32]), "0");

        assert_eq!(
            emit([
                0_u32, 1_u32, 2_u32, 3_u32, 4_u32, 5_u32, 6_u32, 7_u32, 8_u32, 9_u32, 10_u32
            ]),
            "0-10"
        );
    }
}
