use std::num::NonZero;
use std::panic::{RefUnwindSafe, UnwindSafe};

use jiff::Timestamp;
use jiff::civil::Date;
use jiff::tz::TimeZone;
use ohno::AppError;
use reqwest::Url;
use tick::Clock;

use crate::model::{Instance, Repository};

/// Repository-scoped title identity, separate from issue body and update date.
#[derive(Clone, Debug)]
pub(crate) enum IssueIdentity {
    Rolling(Instance),
    Alert(Instance, NonZero<u64>),
}

impl IssueIdentity {
    pub(crate) fn phrase(&self) -> String {
        match self {
            Self::Rolling(instance) => {
                format!("Benchmark history findings for {}", instance.as_str())
            }
            Self::Alert(instance, run) => {
                format!(
                    "Benchmark history workflow failed for {} (run {run})",
                    instance.as_str()
                )
            }
        }
    }

    pub(crate) fn includes_closed(&self) -> bool {
        matches!(self, Self::Alert(..))
    }

    pub(crate) fn matches(&self, title: &str) -> bool {
        let phrase = self.phrase();
        match self {
            Self::Alert(..) => title == phrase,
            Self::Rolling(_) => title
                .strip_prefix(&format!("{phrase} (updated "))
                .and_then(|value| value.strip_suffix(')'))
                .is_some_and(|value| {
                    // Require the emitted calendar form, not merely any date Jiff can parse.
                    value.len() == "YYYY-MM-DD".len()
                        && value
                            .parse::<Date>()
                            .is_ok_and(|date| date.to_string() == value)
                }),
        }
    }

    pub(crate) fn title(&self, clock: &Clock) -> Result<String, AppError> {
        match self {
            Self::Alert(..) => Ok(self.phrase()),
            Self::Rolling(_) => {
                let now = Timestamp::try_from(clock.system_time())
                    .map_err(InvalidPublicationDate::caused_by)?;
                let date = now.to_zoned(TimeZone::UTC).date();
                Ok(format!("{} (updated {date})", self.phrase()))
            }
        }
    }
}

pub(crate) fn validate_run_url(
    repository: &Repository,
    run: NonZero<u64>,
    value: &str,
) -> Result<(), AppError> {
    let url = Url::parse(value).map_err(InvalidRunUrl::caused_by)?;
    if url.scheme() != "https"
        || url.host_str() != Some("github.com")
        || !url.username().is_empty()
        || url.password().is_some()
        || url.port().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
        || url.path() != format!("/{repository}/actions/runs/{run}")
    {
        return Err(InvalidRunUrl::new().into());
    }
    Ok(())
}

/// A failure link must identify the declared repository and workflow run.
#[ohno::error]
#[display("Run URL must identify the declared GitHub repository and workflow run")]
struct InvalidRunUrl;

/// A clock outside the supported calendar cannot date a publication.
#[ohno::error]
#[display("Publication time is outside the supported calendar")]
struct InvalidPublicationDate;

// These diagnostic leaves have no observable mutation.
impl UnwindSafe for InvalidRunUrl {}
impl RefUnwindSafe for InvalidRunUrl {}
impl UnwindSafe for InvalidPublicationDate {}
impl RefUnwindSafe for InvalidPublicationDate {}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use std::time::SystemTime;

    use super::*;

    pub(crate) fn clock(value: &str) -> Clock {
        Clock::new_frozen_at(SystemTime::from(value.parse::<Timestamp>().unwrap()))
    }

    #[test]
    fn rolling_dates_use_utc_across_day_year_and_leap_boundaries() {
        let identity = IssueIdentity::Rolling("project".parse().unwrap());
        for (instant, date) in [
            ("2026-01-01T00:30:00+01:00", "2025-12-31"),
            ("2024-03-01T00:30:00+01:00", "2024-02-29"),
            ("2026-09-17T23:59:59Z", "2026-09-17"),
            ("2026-09-18T00:00:00Z", "2026-09-18"),
        ] {
            let title = identity.title(&clock(instant)).unwrap();
            assert_eq!(
                title,
                format!("Benchmark history findings for project (updated {date})")
            );
            assert!(identity.matches(&title));
        }
    }

    #[test]
    fn exact_identity_excludes_similar_projects_and_invalid_dates() {
        let identity = IssueIdentity::Rolling("project".parse().unwrap());
        assert_eq!(identity.phrase(), "Benchmark history findings for project");
        assert!(!identity.includes_closed());
        for title in [
            "Benchmark history findings for project.extra (updated 2026-09-17)",
            "Benchmark history findings for project (updated 2026-02-30)",
            "Benchmark history findings for project (updated 2026-09-17) extra",
            "Benchmark history findings for project",
        ] {
            assert!(!identity.matches(title));
        }
        let alert = IssueIdentity::Alert("project".parse().unwrap(), NonZero::new(42).unwrap());
        assert!(alert.includes_closed());
        assert!(alert.matches("Benchmark history workflow failed for project (run 42)"));
        assert!(!alert.matches("Benchmark history workflow failed for project (run 420)"));
    }

    #[test]
    fn run_urls_bind_repository_run_and_origin() {
        let repository = "owner/repo".parse().unwrap();
        let run = NonZero::new(42).unwrap();
        validate_run_url(
            &repository,
            run,
            "https://github.com/owner/repo/actions/runs/42",
        )
        .unwrap();
        for url in [
            "https://github.com/owner/other/actions/runs/42",
            "https://github.com/owner/repo/actions/runs/43",
            "https://example.test/owner/repo/actions/runs/42",
            "http://github.com/owner/repo/actions/runs/42",
            "https://name@github.com/owner/repo/actions/runs/42",
            "https://:password@github.com/owner/repo/actions/runs/42",
            "https://github.com:8443/owner/repo/actions/runs/42",
            "https://github.com/owner/repo/actions/runs/42?x=1",
            "https://github.com/owner/repo/actions/runs/42#x",
            "invalid",
        ] {
            validate_run_url(&repository, run, url).unwrap_err();
        }
    }
}
