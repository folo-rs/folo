use std::ops::RangeInclusive;

use cbh_command::{CustomPrincipalType, SetupAzureOptions};
use ohno::AppError;
use serde_json::{Value, from_str, to_string_pretty};

use crate::commands::setup_azure::errors::{SetupParameterError, SetupParametersEncodingError};

/// A named UTF-8 input in the self-contained deployment bundle.
#[derive(Debug)]
pub(crate) struct BundleFile {
    pub(crate) name: &'static str,
    pub(crate) contents: String,
}

// All compile-time inputs live under src, so registry-source builds use exactly
// the same deployment policy as exports and Folo's deployment wrappers.
const ASSETS: &[(&str, &str)] = &[
    ("main.bicep", include_str!("../../azure_bundle/main.bicep")),
    (
        "storage-bootstrap.bicep",
        include_str!("../../azure_bundle/storage-bootstrap.bicep"),
    ),
    (
        "container-bootstrap.bicep",
        include_str!("../../azure_bundle/container-bootstrap.bicep"),
    ),
    ("deploy.ps1", include_str!("../../azure_bundle/deploy.ps1")),
    (
        "AzureDeployment.psm1",
        include_str!("../../azure_bundle/AzureDeployment.psm1"),
    ),
    ("README.md", include_str!("../../azure_bundle/README.md")),
];
const PARAMETER_TEMPLATE: &str = include_str!("../../azure_bundle/parameters.json");
// Azure Storage resource-name length constraints.
const ACCOUNT_NAME_LENGTH: RangeInclusive<usize> = 3..=24;
const CONTAINER_NAME_LENGTH: RangeInclusive<usize> = 3..=63;

/// Builds the deployment driver's literal inputs before either mode performs I/O.
///
/// Export and execution share this validation boundary, including callers that bypass
/// CLI parsing. Supplied values populate JSON data, never executable PowerShell text.
pub(crate) fn prepare(options: &SetupAzureOptions) -> Result<Vec<BundleFile>, AppError> {
    if options.current_user
        && (options.out_dir.is_some()
            || options.custom_principal_id.is_some()
            || options.custom_principal_type.is_some())
    {
        return Err(SetupParameterError::new(
            "CurrentUser",
            "requires execution mode without an explicit custom principal",
        )
        .into());
    }
    let values = [
        ("SubscriptionId", options.subscription_id.as_deref()),
        ("ResourceGroup", options.resource_group.as_deref()),
        ("Location", options.location.as_deref()),
        ("StorageAccountName", options.storage_account.as_deref()),
        ("GithubOrg", options.github_owner.as_deref()),
        ("GithubRepo", options.github_repository.as_deref()),
        ("HistoryBranch", options.history_branch.as_deref()),
    ];
    for (name, value) in values {
        if options.out_dir.is_none() && value.is_none() {
            return Err(SetupParameterError::new(name, "an explicit value is required").into());
        }
        validate_text(name, value)?;
    }
    validate_text("HistoryContainerName", options.container.as_deref())?;
    validate_text("ManagedIdentityName", options.managed_identity.as_deref())?;
    validate_text("CustomPrincipalId", options.custom_principal_id.as_deref())?;
    if options.custom_principal_id.is_some() != options.custom_principal_type.is_some() {
        return Err(SetupParameterError::new(
            "CustomPrincipalId/CustomPrincipalType",
            "supply both the principal ID and its type",
        )
        .into());
    }
    if let Some(account) = &options.storage_account
        && (!ACCOUNT_NAME_LENGTH.contains(&account.len())
            || !account
                .bytes()
                .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit()))
    {
        return Err(SetupParameterError::new(
            "StorageAccountName",
            "use a lowercase alphanumeric Azure storage account name",
        )
        .into());
    }
    if let Some(container) = &options.container
        && (!CONTAINER_NAME_LENGTH.contains(&container.len())
            || container.starts_with('-')
            || container.ends_with('-')
            || container.contains("--")
            || !container
                .bytes()
                .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-'))
    {
        return Err(SetupParameterError::new(
            "HistoryContainerName",
            "use a lowercase Azure container name",
        )
        .into());
    }
    let mut parameters: Value =
        from_str(PARAMETER_TEMPLATE).map_err(SetupParametersEncodingError::caused_by)?;
    for (name, value) in values.into_iter().chain([
        ("HistoryContainerName", options.container.as_deref()),
        ("ManagedIdentityName", options.managed_identity.as_deref()),
        ("CustomPrincipalId", options.custom_principal_id.as_deref()),
        (
            "CustomPrincipalType",
            options.custom_principal_type.map(|kind| match kind {
                CustomPrincipalType::User => "User",
                CustomPrincipalType::Group => "Group",
            }),
        ),
    ]) {
        if let Some(value) = value {
            *parameters
                .get_mut(name)
                .ok_or_else(SetupParametersEncodingError::new)? = Value::String(value.to_owned());
        }
    }
    let mut files = ASSETS
        .iter()
        .map(|&(name, contents)| BundleFile {
            name,
            contents: contents.to_owned(),
        })
        .collect::<Vec<_>>();
    files.push(BundleFile {
        name: "parameters.json",
        contents: to_string_pretty(&parameters).map_err(SetupParametersEncodingError::caused_by)?,
    });
    Ok(files)
}

/// Screens supplied parameter text while leaving export omissions for later completion.
///
/// Bundle preparation uses this independently of execution's required-value checks,
/// so partially populated exports receive the same text validation as deployments.
fn validate_text(name: &'static str, value: Option<&str>) -> Result<(), AppError> {
    if value.is_some_and(|value| value.trim().is_empty() || value.chars().any(char::is_control)) {
        return Err(
            SetupParameterError::new(name, "use nonempty text without control characters").into(),
        );
    }
    Ok(())
}
