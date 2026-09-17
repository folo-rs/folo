use std::path::PathBuf;

use cbh_command::{LocalPrincipalType, SetupAzureOptions};
use clap::{Args, ValueEnum};

/// Standalone provisioning inputs, deliberately independent of benchmark configuration.
#[derive(Args, Debug)]
pub(crate) struct SetupAzureCommand {
    /// Export only, without running tools or checking credentials. Directory must be absent or empty.
    #[arg(long, value_name = "PATH", help_heading = "Output")]
    out_dir: Option<PathBuf>,
    /// Explicit Azure subscription ID.
    #[arg(
        long,
        required_unless_present = "out_dir",
        help_heading = "Azure resources"
    )]
    subscription_id: Option<String>,
    /// Resource group to create or reuse.
    #[arg(
        long,
        required_unless_present = "out_dir",
        help_heading = "Azure resources"
    )]
    resource_group: Option<String>,
    /// Azure region for newly provisioned resources.
    #[arg(
        long,
        required_unless_present = "out_dir",
        help_heading = "Azure resources"
    )]
    location: Option<String>,
    /// Globally unique storage account name.
    #[arg(
        long,
        required_unless_present = "out_dir",
        help_heading = "Azure resources"
    )]
    storage_account: Option<String>,
    /// History container (default: bench-history).
    #[arg(long, help_heading = "Azure resources")]
    container: Option<String>,
    /// Managed identity name (default: id-<storage-account>-bench-history).
    #[arg(long, help_heading = "Azure resources")]
    managed_identity: Option<String>,
    /// GitHub organization or user owning the repository.
    #[arg(
        long,
        required_unless_present = "out_dir",
        help_heading = "GitHub federation"
    )]
    github_owner: Option<String>,
    /// GitHub repository name, without the owner.
    #[arg(
        long,
        required_unless_present = "out_dir",
        help_heading = "GitHub federation"
    )]
    github_repository: Option<String>,
    /// History branch allowed to use the identity, alongside pull requests.
    #[arg(
        long,
        required_unless_present = "out_dir",
        help_heading = "GitHub federation"
    )]
    history_branch: Option<String>,
    /// Optional local Entra principal object ID to grant contributor access.
    #[arg(long, requires = "local_principal_type", help_heading = "Local access")]
    local_principal_id: Option<String>,
    /// Type of the optional local principal.
    #[arg(
        long,
        value_enum,
        requires = "local_principal_id",
        help_heading = "Local access"
    )]
    local_principal_type: Option<PrincipalType>,
    /// Emit explanatory deployment diagnostics.
    #[arg(long, help_heading = "Output")]
    verbose: bool,
}

impl SetupAzureCommand {
    pub(crate) fn into_options(self) -> SetupAzureOptions {
        SetupAzureOptions {
            out_dir: self.out_dir,
            subscription_id: self.subscription_id,
            resource_group: self.resource_group,
            location: self.location,
            storage_account: self.storage_account,
            github_owner: self.github_owner,
            github_repository: self.github_repository,
            history_branch: self.history_branch,
            container: self.container,
            managed_identity: self.managed_identity,
            local_principal_id: self.local_principal_id,
            local_principal_type: self.local_principal_type.map(|kind| match kind {
                PrincipalType::User => LocalPrincipalType::User,
                PrincipalType::Group => LocalPrincipalType::Group,
            }),
            verbose: self.verbose,
        }
    }
}

/// CLI spelling of the optional Entra principal kind.
#[derive(Clone, Copy, Debug, ValueEnum)]
enum PrincipalType {
    User,
    Group,
}

#[cfg(test)]
mod tests {
    use cbh_command::Command;

    use super::*;
    use crate::cli::tests::from_args;

    #[test]
    fn export_needs_no_deployment_inputs() {
        let command = from_args(&["cbh"], &["setup-azure", "--out-dir", "bundle"])
            .unwrap()
            .into_command();
        assert_eq!(
            command,
            Command::SetupAzure(SetupAzureOptions {
                out_dir: Some(PathBuf::from("bundle")),
                ..SetupAzureOptions::default()
            })
        );
    }

    #[test]
    fn execute_requires_explicit_placement() {
        from_args(&["cbh"], &["setup-azure"]).unwrap_err();
    }

    #[test]
    fn maps_explicit_deployment_and_local_access_values() {
        let command = from_args(
            &["cbh"],
            &[
                "setup-azure",
                "--subscription-id",
                "subscription",
                "--resource-group",
                "group",
                "--location",
                "region",
                "--storage-account",
                "account",
                "--github-owner",
                "owner",
                "--github-repository",
                "repository",
                "--history-branch",
                "history/main",
                "--container",
                "container",
                "--managed-identity",
                "identity",
                "--local-principal-id",
                "principal",
                "--local-principal-type",
                "group",
                "--verbose",
            ],
        )
        .unwrap()
        .into_command();
        assert_eq!(
            command,
            Command::SetupAzure(SetupAzureOptions {
                subscription_id: Some("subscription".into()),
                resource_group: Some("group".into()),
                location: Some("region".into()),
                storage_account: Some("account".into()),
                github_owner: Some("owner".into()),
                github_repository: Some("repository".into()),
                history_branch: Some("history/main".into()),
                container: Some("container".into()),
                managed_identity: Some("identity".into()),
                local_principal_id: Some("principal".into()),
                local_principal_type: Some(LocalPrincipalType::Group),
                verbose: true,
                out_dir: None,
            })
        );
    }

    #[test]
    fn maps_local_user_access() {
        let Command::SetupAzure(options) = from_args(
            &["cbh"],
            &[
                "setup-azure",
                "--out-dir",
                "bundle",
                "--local-principal-id",
                "principal",
                "--local-principal-type",
                "user",
            ],
        )
        .unwrap()
        .into_command() else {
            panic!()
        };
        assert_eq!(options.local_principal_type, Some(LocalPrincipalType::User));
    }

    #[test]
    fn local_access_requires_both_fields() {
        for flag in ["--local-principal-id", "--local-principal-type"] {
            let value = if flag == "--local-principal-id" {
                "id"
            } else {
                "user"
            };
            from_args(
                &["cbh"],
                &["setup-azure", "--out-dir", "bundle", flag, value],
            )
            .unwrap_err();
        }
    }
}
