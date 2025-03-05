use itertools::{FoldWhile, Itertools};

use std::collections::VecDeque;

use crate::Item;

/// Generates a cpulist in a format that can be parsed by [`parse()`][crate::parse].
///
/// The exact emitted representation is unspecified and may change across versions of this crate.
/// All we promise is that it is a recognizable cpulist and can be parsed by this crate.
pub fn emit<'a>(items: impl IntoIterator<Item = &'a Item>) -> String {
    // We group consecutive items to generate shorter output strings.
    // Sorted remaining items that we have not yet grouped.
    let mut remaining = items
        .into_iter()
        .unique()
        .sorted_unstable()
        .collect::<VecDeque<_>>();

    // We want to coalesce consecutive numbers into groups (ranges).
    // Each group is (start ID, len).
    let mut groups: Vec<(Item, Item)> = Vec::new();

    while !remaining.is_empty() {
        let group = remaining
            .iter()
            .fold_while(None, |acc: Option<(Item, Item)>, p: &&Item| {
                if let Some((start, len)) = acc {
                    if start + len == **p {
                        // This item is part of the current group.
                        FoldWhile::Continue(Some((start, len + 1)))
                    } else {
                        // This item is not part of the current group.
                        FoldWhile::Done(Some((start, len)))
                    }
                } else {
                    // Start a new group.
                    FoldWhile::Continue(Some((**p, 1)))
                }
            });

        let (start, len) = group
            .into_inner()
            .expect("this must be Some if we still have remaining items");

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

    #[test]
    fn emit_smoke_test() {
        assert_eq!(emit(&[]), "");

        assert_eq!(emit(&[555]), "555");

        assert_eq!(emit(&[555, 666]), "555,666");

        assert_eq!(emit(&[0, 1, 2, 3]), "0-3");

        assert_eq!(emit(&[0, 1, 2, 3, 6, 7, 8, 11, 12, 13]), "0-3,6-8,11-13");

        assert_eq!(emit(&[1, 2, 3]), "1-3");

        assert_eq!(emit(&[0, 1, 2, 3, 4, 5, 6]), "0-6");

        assert_eq!(emit(&[0]), "0");

        assert_eq!(emit(&[0, 1, 3]), "0,1,3");

        assert_eq!(emit(&[0, 3, 5, 6, 8, 9, 11, 14]), "0,3,5,6,8,9,11,14");

        assert_eq!(emit(&[0]), "0");

        assert_eq!(emit(&[0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10]), "0-10");
    }
}
