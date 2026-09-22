use std::collections::BTreeMap;
use std::path::Path;

use jiff::tz::TimeZone;
use jiff::{Span, Timestamp};
use ohno::AppError;
use tick::Clock;

use crate::action::errors::{InvalidInput, InvalidOutput};
use crate::action::execute::git;
#[cfg(any(test, feature = "private-test-util"))]
use crate::action::native::NativeHost;
use crate::action::port::Host;
#[cfg(any(test, feature = "private-test-util"))]
use crate::action::preparation::Flow;
use crate::action::preparation::execute::resolve;
#[cfg(any(test, feature = "private-test-util"))]
use crate::action::preparation::inputs::WorkflowInputs;
use crate::model::CommitSha;

/// Validated exact references or calendar bounds for historical range preparation.
///
/// The variants exclude incompatible caller inputs before Git or output operations begin.
pub(crate) enum BackfillInput {
    Exact {
        from: String,
        to: String,
    },
    Rolling {
        lookback: Span,
        minimum_age: Span,
        to: Option<String>,
    },
}

impl BackfillInput {
    pub(crate) fn parse(values: &BTreeMap<String, String>) -> Result<Self, AppError> {
        if values.contains_key("from") {
            for key in ["lookback", "minimum-age"] {
                if values.contains_key(key) {
                    return Err(InvalidInput::new(
                        key,
                        "rolling inputs cannot accompany an exact range",
                    )
                    .into());
                }
            }
            return Ok(Self::Exact {
                from: reference(values, "from")?,
                to: reference(values, "to")?,
            });
        }
        Ok(Self::Rolling {
            lookback: duration(values, "lookback")?,
            minimum_age: duration(values, "minimum-age")?,
            to: values
                .contains_key("to")
                .then(|| reference(values, "to"))
                .transpose()?,
        })
    }

    pub(crate) async fn select(
        &self,
        host: &impl Host,
        cwd: &Path,
        head: &CommitSha,
        clock: &Clock,
    ) -> Result<Option<(CommitSha, CommitSha)>, AppError> {
        let (lookback, minimum_age, override_to) = match self {
            Self::Exact { from, to } => {
                return Ok(Some((
                    resolve(host, cwd, from).await?,
                    resolve(host, cwd, to).await?,
                )));
            }
            Self::Rolling {
                lookback,
                minimum_age,
                to,
            } => (*lookback, *minimum_age, to),
        };

        let now = Timestamp::try_from(clock.system_time()).map_err(|error| {
            InvalidOutput::caused_by(
                "preparation clock is outside the supported date range",
                error,
            )
        })?;
        let since = cutoff(lookback, "lookback", now)?;
        let until = cutoff(minimum_age, "minimum-age", now)?;
        let to = if let Some(reference) = override_to {
            Some(resolve(host, cwd, reference).await?)
        } else if until < Timestamp::UNIX_EPOCH {
            // Git stores nonnegative Unix commit timestamps. Its human date parser must not
            // reinterpret an earlier UTC calendar cutoff as a date in another year.
            None
        } else {
            let selected = git(
                host,
                cwd,
                &[
                    "rev-list",
                    "--first-parent",
                    "--max-count=1",
                    &format!("--min-age={}", until.as_second()),
                    head.as_str(),
                    "--",
                ],
            )
            .await?;
            (!selected.trim().is_empty())
                .then(|| selected.trim().parse())
                .transpose()?
        };
        let Some(to) = to else {
            host.note(&format!(
                "No backfill endpoint is eligible on {} at or before {until}; \
                 minimum-age={minimum_age} is measured from {now}.",
                head.as_str()
            ));
            return Ok(None);
        };
        // Preserve Git's date-filter traversal, including its early stop on old ancestors.
        // Numeric bounds avoid Git's approximate date parser; a pre-epoch start includes all dates.
        // Ref: docs/implementation.md, Workflow preparation.
        let commits = git(
            host,
            cwd,
            &[
                "rev-list",
                "--first-parent",
                &format!("--max-age={}", git_since(since)),
                to.as_str(),
                "--",
            ],
        )
        .await?;
        let from = commits
            .lines()
            .try_fold(to.clone(), |_, line| line.parse::<CommitSha>())?;
        host.note(&format!(
            "Selected backfill {}..{} using lookback={lookback} from {now} \
             (cutoff {since}); minimum-age={minimum_age} gives cutoff {until}. \
             Explicit to={override_to:?} bypasses age selection when present; \
             an empty lookback window retains only to.",
            from.as_str(),
            to.as_str(),
        ));
        Ok(Some((from, to)))
    }
}

/// Runs the native Git range planner with an explicit checkout and clock.
///
/// This unsupported test entry point bypasses event/configuration discovery, not range selection.
///
/// # Errors
///
/// Returns an error for invalid range inputs, clock values, commit references or Git operations.
#[cfg(any(test, feature = "private-test-util"))]
#[cfg_attr(test, mutants::skip)] // Native adapter wiring is covered by real-Git integration tests.
pub async fn prepare_backfill_at(
    inputs_json: &[u8],
    cwd: &Path,
    head: &str,
    clock: &Clock,
) -> Result<Option<(String, String)>, AppError> {
    let inputs = WorkflowInputs::parse(inputs_json, Flow::Backfill)?;
    let range = inputs
        .backfill
        .expect("the backfill flow validates its exact or rolling selection");
    Ok(range
        .select(&NativeHost, cwd, &head.parse()?, clock)
        .await?
        .map(|(from, to)| (from.as_str().to_owned(), to.as_str().to_owned())))
}

fn required<'a>(values: &'a BTreeMap<String, String>, key: &str) -> Result<&'a str, AppError> {
    values
        .get(key)
        .filter(|value| !value.is_empty())
        .map(String::as_str)
        .ok_or_else(|| InvalidInput::new(key, "a nonblank backfill input is required").into())
}

fn reference(values: &BTreeMap<String, String>, key: &str) -> Result<String, AppError> {
    let value = required(values, key)?;
    if value.starts_with('-') {
        return Err(InvalidInput::new(key, "expected a commit reference, not an option").into());
    }
    Ok(value.to_owned())
}

fn duration(values: &BTreeMap<String, String>, key: &str) -> Result<Span, AppError> {
    let value = required(values, key)?;
    let span = value.parse::<Span>().map_err(|error| {
        InvalidInput::caused_by(
            key,
            "expected a friendly or ISO duration, not a date",
            error,
        )
    })?;
    if key == "lookback" && span.is_zero() {
        return Err(InvalidInput::new(key, "lookback must be nonzero").into());
    }
    Ok(span.abs())
}

fn cutoff(span: Span, key: &str, now: Timestamp) -> Result<Timestamp, AppError> {
    now.to_zoned(TimeZone::UTC)
        .checked_sub(span)
        .map(|zoned| zoned.timestamp())
        .map_err(|error| {
            InvalidInput::caused_by(key, "duration exceeds the supported calendar range", error)
                .into()
        })
}

/// Converts an inclusive lower cutoff to Git's whole-second commit precision.
fn git_since(since: Timestamp) -> i64 {
    let since = since.max(Timestamp::UNIX_EPOCH);
    let seconds = since.as_second();
    if since.subsec_nanosecond() == 0 {
        seconds
    } else {
        // Rounding down would admit commits before a fractional lower bound.
        // Ref: docs/implementation.md, Workflow preparation.
        seconds
            .checked_add(1)
            .expect("Jiff's bounded calendar range leaves room for one more second in i64")
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;

    #[test]
    fn git_since_rounds_up_without_losing_boundary_precision() {
        assert_eq!(git_since(Timestamp::UNIX_EPOCH), 0);
        assert_eq!(git_since(Timestamp::from_second(1).unwrap()), 1);
        assert_eq!(git_since(Timestamp::new(1, 1).unwrap()), 2);
        assert_eq!(git_since(Timestamp::new(0, -1).unwrap()), 0);
        assert_eq!(git_since(Timestamp::MAX), Timestamp::MAX.as_second() + 1);
    }
}
