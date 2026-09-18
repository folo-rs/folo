use std::path::PathBuf;

/// Inputs for standalone Azure provisioning or an export-only deployment bundle.
#[doc(hidden)]
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct SetupAzureOptions {
    /// Export without executing tools.
    ///
    /// Relative paths resolve against the invocation directory.
    pub out_dir: Option<PathBuf>,
    /// Explicit target subscription, never inferred from the active CLI subscription.
    pub subscription_id: Option<String>,
    /// Resource group to create or reuse.
    pub resource_group: Option<String>,
    /// Region for newly provisioned resources.
    pub location: Option<String>,
    /// Storage account to create or reuse.
    pub storage_account: Option<String>,
    /// GitHub organization or user owning the repository.
    pub github_owner: Option<String>,
    /// GitHub repository name without the owner.
    pub github_repository: Option<String>,
    /// Branch whose history workflows may use the managed identity.
    pub history_branch: Option<String>,
    /// Container override; omission selects the standard history container.
    pub container: Option<String>,
    /// Identity override; omission derives a name from the selected account.
    pub managed_identity: Option<String>,
    /// Optional local principal receiving Storage Blob Data Contributor access.
    pub local_principal_id: Option<String>,
    /// Type of the optional local principal, supplied together with its ID.
    pub local_principal_type: Option<LocalPrincipalType>,
    /// Emit explanatory deployment diagnostics.
    pub verbose: bool,
}

/// The local Entra principal to which optional history access is granted.
#[doc(hidden)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LocalPrincipalType {
    /// An individual maintainer.
    User,
    /// A group of maintainers.
    Group,
}
