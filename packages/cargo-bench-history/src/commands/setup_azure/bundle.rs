use std::ops::RangeInclusive;

use cbh_command::{LocalPrincipalType, SetupAzureOptions};
use ohno::AppError;
use serde_json::Value;

use crate::commands::setup_azure::errors::{SetupParameterError, SetupParametersEncodingError};

/// A named UTF-8 input in the self-contained deployment bundle.
#[derive(Debug)]
pub(crate) struct BundleFile {
    pub(crate) name: &'static str,
    pub(crate) contents: String,
}

// All compile-time inputs live under src, so registry-source builds use exactly
// the same deployment policy as exports and Folo's standalone wrapper.
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
        "ProductionIdentityDeployment.psm1",
        include_str!("../../azure_bundle/ProductionIdentityDeployment.psm1"),
    ),
    ("README.md", include_str!("../../azure_bundle/README.md")),
];
const PARAMETER_TEMPLATE: &str = include_str!("../../azure_bundle/parameters.json");
// Azure Storage resource-name length constraints.
const ACCOUNT_NAME_LENGTH: RangeInclusive<usize> = 3..=24;
const CONTAINER_NAME_LENGTH: RangeInclusive<usize> = 3..=63;

pub(crate) fn prepare(options: &SetupAzureOptions) -> Result<Vec<BundleFile>, AppError> {
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
    validate_text("LocalPrincipalId", options.local_principal_id.as_deref())?;
    if options.local_principal_id.is_some() != options.local_principal_type.is_some() {
        return Err(SetupParameterError::new(
            "LocalPrincipalId/LocalPrincipalType",
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
    let mut parameters: Value = serde_json::from_str(PARAMETER_TEMPLATE)
        .map_err(SetupParametersEncodingError::caused_by)?;
    for (name, value) in values.into_iter().chain([
        ("HistoryContainerName", options.container.as_deref()),
        ("ManagedIdentityName", options.managed_identity.as_deref()),
        ("LocalPrincipalId", options.local_principal_id.as_deref()),
        (
            "LocalPrincipalType",
            options.local_principal_type.map(|kind| match kind {
                LocalPrincipalType::User => "User",
                LocalPrincipalType::Group => "Group",
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
        contents: serde_json::to_string_pretty(&parameters)
            .map_err(SetupParametersEncodingError::caused_by)?,
    });
    Ok(files)
}

fn validate_text(name: &'static str, value: Option<&str>) -> Result<(), AppError> {
    if value.is_some_and(|value| value.trim().is_empty() || value.chars().any(char::is_control)) {
        return Err(
            SetupParameterError::new(name, "use nonempty text without control characters").into(),
        );
    }
    Ok(())
}
