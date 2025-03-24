use itertools::Itertools;

use crate::Item;

/// Parses a [cpulist][crate] and returns the numeric items in ascending order, removing duplicates.
///
/// An empty string is valid input and returns an empty result.
///
/// See [crate-level documentation][crate] for details.
pub fn parse(cpulist: &str) -> crate::Result<Vec<Item>> {
    let parts = cpulist.split(',');

    let item_ranges: crate::Result<Vec<Vec<Item>>> = parts.map(parse_part).collect();

    item_ranges.map(|x| x.into_iter().flatten().sorted().dedup().collect())
}

fn parse_part(part: &str) -> crate::Result<Vec<Item>> {
    if part.is_empty() {
        return Ok(vec![]);
    }

    if let Some((range_start, range_end_inc)) = part.split_once('-') {
        parse_range(range_start, range_end_inc)
    } else {
        parse_single(part).map(|item| vec![item])
    }
}

fn parse_range(range_start: &str, range_end_inc: &str) -> crate::Result<Vec<Item>> {
    // This is a range, with optional stride.
    let range_start = range_start
        .parse::<Item>()
        .map_err(|inner| crate::Error::InvalidSyntax {
            invalid_value: range_start.to_string(),
            problem: format!("range start could not be parsed as an integer: {inner}"),
        })?;

    // If no stride is specified, we just default to 1 and pretend it was specified.
    let (range_end_inc, stride) =
        if let Some((range_end_inc, stride)) = range_end_inc.split_once(':') {
            (
                range_end_inc
                    .parse::<Item>()
                    .map_err(|inner| crate::Error::InvalidSyntax {
                        invalid_value: range_end_inc.to_string(),
                        problem: format!("range end could not be parsed as an integer: {inner}"),
                    })?,
                stride
                    .parse::<Item>()
                    .map_err(|inner| crate::Error::InvalidSyntax {
                        invalid_value: stride.to_string(),
                        problem: format!("range stride could not be parsed as an integer: {inner}"),
                    })?,
            )
        } else {
            (
                range_end_inc
                    .parse::<Item>()
                    .map_err(|inner| crate::Error::InvalidSyntax {
                        invalid_value: range_end_inc.to_string(),
                        problem: format!("range end could not be parsed as an integer: {inner}"),
                    })?,
                1,
            )
        };

    if stride == 0 {
        return Err(crate::Error::InvalidSyntax {
            invalid_value: stride.to_string(),
            problem: "range stride must not be zero".to_string(),
        });
    }

    if range_start > range_end_inc {
        return Err(crate::Error::InvalidSyntax {
            invalid_value: format!("{}-{}", range_start, range_end_inc),
            problem: "range start must be <= end".to_string(),
        });
    }

    Ok((range_start..=range_end_inc)
        .step_by(stride as usize)
        .collect())
}

fn parse_single(single_item_part: &str) -> crate::Result<Item> {
    single_item_part
        .parse::<Item>()
        .map_err(|inner| crate::Error::InvalidSyntax {
            invalid_value: single_item_part.to_string(),
            problem: format!(
                "part was not a range but could not be parsed as an integer either: {inner}"
            ),
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_smoke_test() {
        assert_eq!(parse("").unwrap(), vec![]);

        assert_eq!(parse("555").unwrap(), vec![555]);

        assert_eq!(parse("0,1,2,3").unwrap(), vec![0, 1, 2, 3]);

        assert_eq!(parse("2,3,1").unwrap(), vec![1, 2, 3]);

        assert_eq!(parse("0-5,1-6").unwrap(), vec![0, 1, 2, 3, 4, 5, 6]);

        assert_eq!(parse("0-0:5").unwrap(), vec![0]);

        assert_eq!(parse("0-0,1-1,3-3").unwrap(), vec![0, 1, 3]);
        assert_eq!(
            parse("0-10:3,5-15:3").unwrap(),
            vec![0, 3, 5, 6, 8, 9, 11, 14]
        );

        assert_eq!(parse("0-10:999999").unwrap(), vec![0]);

        assert_eq!(
            parse("0,1,2-4,5-9:2,6-10:2").unwrap(),
            vec![0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10]
        );
    }

    #[test]
    fn zero_stride_is_error() {
        parse("1-22:0").unwrap_err();
    }

    #[test]
    fn range_direction_fail_is_error() {
        parse("2-1").unwrap_err();
    }

    #[test]
    fn garbage_is_error() {
        parse("foo").unwrap_err();
        parse("123-foo").unwrap_err();
        parse("foo-123").unwrap_err();
        parse("123-456:foo").unwrap_err();
        parse("123-foo:456").unwrap_err();
    }
}
