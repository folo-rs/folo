use std::path::PathBuf;

use cbh_command::{CustomPrincipalType, SetupAzureOptions};
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
    /// Existing Entra user/group object ID to grant account-scoped blob contributor access.
    #[arg(
        long,
        requires = "custom_principal_type",
        help_heading = "Additional access"
    )]
    custom_principal_id: Option<String>,
    /// Type of the custom principal.
    #[arg(
        long,
        value_enum,
        requires = "custom_principal_id",
        help_heading = "Additional access"
    )]
    custom_principal_type: Option<PrincipalType>,
    /// Grant the Azure CLI signed-in user access in the selected subscription's tenant.
    ///
    /// The selected subscription must also be active in Azure CLI for directory lookup.
    #[arg(
        long,
        conflicts_with_all = ["out_dir", "custom_principal_id", "custom_principal_type"],
        help_heading = "Additional access"
    )]
    current_user: bool,
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
            custom_principal_id: self.custom_principal_id,
            custom_principal_type: self.custom_principal_type.map(|kind| match kind {
                PrincipalType::User => CustomPrincipalType::User,
                PrincipalType::Group => CustomPrincipalType::Group,
            }),
            current_user: self.current_user,
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
    fn maps_explicit_deployment_and_custom_access_values() {
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
                "--custom-principal-id",
                "principal",
                "--custom-principal-type",
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
                custom_principal_id: Some("principal".into()),
                custom_principal_type: Some(CustomPrincipalType::Group),
                current_user: false,
                verbose: true,
                out_dir: None,
            })
        );
    }

    #[test]
    fn maps_custom_user_access() {
        let Command::SetupAzure(options) = from_args(
            &["cbh"],
            &[
                "setup-azure",
                "--out-dir",
                "bundle",
                "--custom-principal-id",
                "principal",
                "--custom-principal-type",
                "user",
            ],
        )
        .unwrap()
        .into_command() else {
            panic!()
        };
        assert_eq!(
            options.custom_principal_type,
            Some(CustomPrincipalType::User)
        );
    }

    #[test]
    fn custom_access_requires_type() {
        from_args(
            &["cbh"],
            &[
                "setup-azure",
                "--out-dir",
                "bundle",
                "--custom-principal-id",
                "id",
            ],
        )
        .unwrap_err();
    }

    #[test]
    fn custom_access_requires_id() {
        from_args(
            &["cbh"],
            &[
                "setup-azure",
                "--out-dir",
                "bundle",
                "--custom-principal-type",
                "user",
            ],
        )
        .unwrap_err();
    }

    // Representative placement keeps access conflicts independent of missing-input errors.
    const CURRENT_USER_INPUTS: &[&str] = &[
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
        "main",
        "--current-user",
    ];

    #[test]
    fn maps_current_user() {
        let Command::SetupAzure(options) = from_args(&["cbh"], CURRENT_USER_INPUTS)
            .unwrap()
            .into_command()
        else {
            panic!()
        };
        assert!(options.current_user);
        assert!(options.custom_principal_id.is_none());
        assert!(options.custom_principal_type.is_none());
    }

    #[test]
    fn current_user_conflicts_with_export() {
        rejects_current_user_conflict(&["--out-dir", "bundle"]);
    }

    #[test]
    fn current_user_conflicts_with_custom_id() {
        rejects_current_user_conflict(&["--custom-principal-id", "id"]);
    }

    #[test]
    fn current_user_conflicts_with_custom_type() {
        rejects_current_user_conflict(&["--custom-principal-type", "user"]);
    }

    #[test]
    fn current_user_conflicts_with_custom_principal() {
        rejects_current_user_conflict(&[
            "--custom-principal-id",
            "id",
            "--custom-principal-type",
            "user",
        ]);
    }

    fn rejects_current_user_conflict(conflicting: &[&str]) {
        let mut inputs = CURRENT_USER_INPUTS.to_vec();
        inputs.extend(conflicting);
        let error = from_args(&["cbh"], &inputs).unwrap_err();
        assert!(error.status.is_err());
    }
}
