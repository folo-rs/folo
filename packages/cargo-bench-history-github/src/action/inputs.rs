use std::collections::BTreeMap;
use std::fmt;
use std::num::NonZero;
use std::str::FromStr;

use ohno::AppError;
use serde::de::{Error as _, MapAccess, Visitor};
use serde::{Deserialize, Deserializer};

use crate::action::errors::InvalidInput;
use crate::action::flags::ENCODED_SEPARATOR;
use crate::result::PlatformCoverage;

/// Validated post-install command inputs consumed by process and publication planning.
///
/// Installation selection belongs to the action bootstrap; this representation contains only
/// inputs whose applicability and basic value forms have been checked for the chosen command.
#[derive(Debug)]
pub(crate) struct Inputs {
    pub(crate) command: ActionCommand,
    values: BTreeMap<String, String>,
}

impl Inputs {
    /// Establishes command-specific validity before orchestration performs benchmark or I/O work.
    pub(crate) fn parse(json: &[u8]) -> Result<Self, AppError> {
        let InputObject(mut values) = serde_json::from_slice(json).map_err(|error| {
            InvalidInput::caused_by("inputs-file", "expected a string object", error)
        })?;
        for key in values.keys() {
            if !ALL_INPUTS.contains(&key.as_str()) {
                return Err(InvalidInput::new(key, "unknown input").into());
            }
        }
        values.retain(|_, value| !value.is_empty());
        let command = values
            .get("command")
            .ok_or_else(|| InvalidInput::new("command", "required"))?
            .parse()?;
        let inputs = Self { command, values };
        inputs.validate()?;
        Ok(inputs)
    }

    /// Exposes a supplied nonempty value without applying a command-independent default.
    pub(crate) fn get(&self, key: &str) -> Option<&str> {
        self.values.get(key).map(String::as_str)
    }

    /// Requires execution data that the selected planner cannot obtain from another source.
    pub(crate) fn required(&self, key: &str) -> Result<&str, AppError> {
        self.get(key)
            .ok_or_else(|| InvalidInput::new(key, "required").into())
    }

    /// Resolves an action Boolean using the calling command's chosen default.
    pub(crate) fn boolean(&self, key: &str, default: bool) -> Result<bool, AppError> {
        match self.get(key) {
            None => Ok(default),
            Some("true") => Ok(true),
            Some("false") => Ok(false),
            Some(_) => Err(InvalidInput::new(key, "expected true or false").into()),
        }
    }

    /// Produces individual argument values from a nonempty, comma-separated scope selection.
    pub(crate) fn list(&self, key: &str) -> Result<Vec<&str>, AppError> {
        self.get(key).map_or_else(
            || Ok(Vec::new()),
            |value| {
                value
                    .split(',')
                    .map(str::trim)
                    .map(|item| {
                        if item.is_empty() || item.starts_with('-') {
                            Err(
                                InvalidInput::new(key, "expected a nonempty comma-separated list")
                                    .into(),
                            )
                        } else {
                            Ok(item)
                        }
                    })
                    .collect()
            },
        )
    }

    /// Keeps intended/completed platform evidence independent of measured-key deduplication.
    pub(crate) fn platforms(&self) -> Result<PlatformCoverage, AppError> {
        PlatformCoverage::parse(
            self.required("expected-platforms")?,
            self.required("completed-platforms")?,
        )
    }

    /// Checks the full selected input group, including state-specific report/scope requirements.
    fn validate(&self) -> Result<(), AppError> {
        for (key, value) in &self.values {
            if !self.command.accepts(key) {
                return Err(InvalidInput::new(key, "does not apply to this command").into());
            }
            if value.trim().is_empty() || value.contains(['\r', '\n', '\0']) {
                return Err(InvalidInput::new(key, "expected a nonblank single-line value").into());
            }
            if key == "rustflags" && value.contains(ENCODED_SEPARATOR) {
                return Err(
                    InvalidInput::new(key, "encoded argument separator is reserved").into(),
                );
            }
        }
        for key in [
            "all-features",
            "no-default-features",
            "ignore-errors",
            "empty-scope",
        ] {
            self.boolean(key, false)?;
        }
        for key in ["packages", "exclude", "bench", "features"] {
            self.list(key)?;
        }
        for key in ["best-of", "pr-number", "run-id", "run-attempt"] {
            if let Some(value) = self.get(key) {
                value.parse::<NonZero<u64>>().map_err(|error| {
                    InvalidInput::caused_by(key, "expected a positive integer", error)
                })?;
            }
        }
        self.conflict("packages", "exclude")?;
        self.conflict("local-path", "cache")?;
        match self.command {
            ActionCommand::Collect | ActionCommand::Backfill => {
                let default = if self.command == ActionCommand::Collect {
                    "error"
                } else {
                    "skip"
                };
                let mode = self.get("on-existing").unwrap_or(default);
                if !matches!(mode, "error" | "skip" | "overwrite")
                    || (self.command == ActionCommand::Backfill && mode == "error")
                {
                    return Err(InvalidInput::new(
                        "on-existing",
                        "unsupported write mode for this command",
                    )
                    .into());
                }
                if self.command == ActionCommand::Backfill {
                    for key in ["from", "to"] {
                        if self.required(key)?.starts_with('-') {
                            return Err(InvalidInput::new(
                                key,
                                "expected a commit reference, not an option",
                            )
                            .into());
                        }
                    }
                }
            }
            ActionCommand::AnalyzeHistory | ActionCommand::AnalyzePr => {
                self.required("machine-keys")?;
                self.platforms()?;
            }
            ActionCommand::Publish(_, state) => {
                let empty = self.boolean("empty-scope", false)?;
                if state == PublishState::Inconclusive && empty {
                    for key in REPORT_INPUTS.iter().copied().chain(["packages"]) {
                        if self.get(key).is_some() {
                            return Err(InvalidInput::new(key, "conflicts with empty-scope").into());
                        }
                    }
                } else if state.is_report() {
                    if self.get("head").is_some() {
                        return Err(InvalidInput::new(
                            "head",
                            "report publication uses analyzed-sha",
                        )
                        .into());
                    }
                    for key in ["body-file", "report-file", "analyzed-sha"] {
                        self.required(key)?;
                    }
                    self.platforms()?;
                }
                if self.command.is_comment()
                    && (state == PublishState::Preflight || (state.is_report() && !empty))
                {
                    self.required("packages")?;
                }
                if state == PublishState::Failed
                    && !matches!(self.required("conclusion")?, "failure" | "cancelled")
                {
                    return Err(
                        InvalidInput::new("conclusion", "expected failure or cancelled").into(),
                    );
                }
            }
            ActionCommand::Alert => {}
        }
        Ok(())
    }

    /// Rejects simultaneous selections rather than silently choosing one planner interpretation.
    fn conflict(&self, left: &str, right: &str) -> Result<(), AppError> {
        if self.get(left).is_some() && self.get(right).is_some() {
            return Err(InvalidInput::new(left, format!("conflicts with {right}")).into());
        }
        Ok(())
    }
}

/// Root commands describe pipeline stages, never caller-selected report vocabulary.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ActionCommand {
    Collect,
    Backfill,
    AnalyzeHistory,
    AnalyzePr,
    Publish(Sink, PublishState),
    Alert,
}

impl ActionCommand {
    /// Identifies publication inputs that must disclose a PR's benchmarked package scope.
    fn is_comment(self) -> bool {
        matches!(self, Self::Publish(Sink::Comment, _))
    }

    /// Defines which supplied fields have meaning for each root command.
    fn accepts(self, key: &str) -> bool {
        if matches!(key, "command" | "working-directory" | "config") {
            return true;
        }
        match self {
            Self::Collect | Self::Backfill => {
                BUILD_INPUTS.contains(&key)
                    || key == "local-path"
                    || (self == Self::Backfill && matches!(key, "from" | "to" | "ignore-errors"))
            }
            Self::AnalyzeHistory | Self::AnalyzePr => {
                matches!(
                    key,
                    "local-path"
                        | "cache"
                        | "machine-keys"
                        | "context"
                        | "expected-platforms"
                        | "completed-platforms"
                ) || (self == Self::AnalyzeHistory && key == "since")
                    || (self == Self::AnalyzePr && key == "base")
            }
            Self::Publish(sink, state) => {
                matches!(key, "run-id" | "run-attempt")
                    || (sink == Sink::Comment && key == "pr-number")
                    || (sink == Sink::Comment && state != PublishState::Failed && key == "packages")
                    || (state.is_report() && REPORT_INPUTS.contains(&key))
                    || (state == PublishState::Inconclusive && key == "empty-scope")
                    || (matches!(
                        state,
                        PublishState::Preflight | PublishState::Failed | PublishState::Inconclusive
                    ) && key == "head")
                    || (state == PublishState::Failed && matches!(key, "run-url" | "conclusion"))
            }
            Self::Alert => matches!(key, "run-id" | "run-url"),
        }
    }
}

impl FromStr for ActionCommand {
    type Err = AppError;

    /// Decodes the fixed root-stage vocabulary without adding aliases or a caller-selected mode.
    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "collect" => Ok(Self::Collect),
            "backfill" => Ok(Self::Backfill),
            "analyze-history" => Ok(Self::AnalyzeHistory),
            "analyze-pr" => Ok(Self::AnalyzePr),
            "alert" => Ok(Self::Alert),
            _ => {
                let Some((sink, state)) = value
                    .strip_prefix("publish-")
                    .and_then(|value| value.split_once('-'))
                else {
                    return Err(InvalidInput::new("command", "unknown command").into());
                };
                let sink = match sink {
                    "comment" => Sink::Comment,
                    "issue" => Sink::Issue,
                    _ => {
                        return Err(InvalidInput::new("command", "unknown publication sink").into());
                    }
                };
                let state = match state {
                    "findings" => PublishState::Findings,
                    "clean" => PublishState::Clean,
                    "preflight" => PublishState::Preflight,
                    "inconclusive" => PublishState::Inconclusive,
                    "failed" => PublishState::Failed,
                    _ => {
                        return Err(
                            InvalidInput::new("command", "unknown publication state").into()
                        );
                    }
                };
                Ok(Self::Publish(sink, state))
            }
        }
    }
}

/// Lifecycle destinations have distinct scope and freshness contracts.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Sink {
    Comment,
    Issue,
}

/// Report states reuse the companion's evidence gates; nonreport states carry ownership.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum PublishState {
    Findings,
    Clean,
    Preflight,
    Inconclusive,
    Failed,
}

impl PublishState {
    /// Identifies states that may require analysis evidence rather than only execution ownership.
    fn is_report(self) -> bool {
        matches!(self, Self::Findings | Self::Clean | Self::Inconclusive)
    }
}

const BUILD_INPUTS: &[&str] = &[
    "packages",
    "exclude",
    "bench",
    "best-of",
    "on-existing",
    "all-features",
    "no-default-features",
    "features",
    "rustflags",
];
const REPORT_INPUTS: &[&str] = &[
    "body-file",
    "report-file",
    "analyzed-sha",
    "artifact-url",
    "expected-platforms",
    "completed-platforms",
];
const ALL_INPUTS: &[&str] = &[
    "command",
    "working-directory",
    "config",
    "local-path",
    "packages",
    "exclude",
    "bench",
    "best-of",
    "on-existing",
    "all-features",
    "no-default-features",
    "features",
    "rustflags",
    "machine-keys",
    "cache",
    "context",
    "base",
    "since",
    "from",
    "to",
    "ignore-errors",
    "body-file",
    "report-file",
    "analyzed-sha",
    "artifact-url",
    "expected-platforms",
    "completed-platforms",
    "pr-number",
    "head",
    "run-id",
    "run-attempt",
    "run-url",
    "conclusion",
    "empty-scope",
];

/// Unlike an ordinary map decoder, this preserves strict rejection of duplicate JSON keys.
pub(crate) struct InputObject(pub(crate) BTreeMap<String, String>);

impl<'de> Deserialize<'de> for InputObject {
    /// Selects object decoding so scalar and array inputs cannot bypass string-field validation.
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        deserializer.deserialize_map(InputVisitor)
    }
}

/// A string-only object decoder keeps YAML serialization mistakes out of command planning.
struct InputVisitor;

impl<'de> Visitor<'de> for InputVisitor {
    type Value = InputObject;

    // This supplies only Serde's diagnostic wording, not input acceptance or rejection.
    // Parser tests verify the error conditions without prescribing human-readable text.
    #[cfg_attr(test, mutants::skip)]
    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("an object with unique keys and string values")
    }

    /// Preserves duplicate-key detection before the raw string object becomes validated inputs.
    fn visit_map<M: MapAccess<'de>>(self, mut map: M) -> Result<Self::Value, M::Error> {
        let mut values = BTreeMap::new();
        while let Some((key, value)) = map.next_entry::<String, String>()? {
            if values.insert(key.clone(), value).is_some() {
                return Err(M::Error::custom(format!("duplicate input {key}")));
            }
        }
        Ok(InputObject(values))
    }
}
