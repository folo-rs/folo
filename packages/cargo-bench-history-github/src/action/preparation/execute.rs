use std::collections::BTreeSet;
use std::fmt::Write as _;
use std::path::{Component, Path};

use cbh_config::rebase;
use ohno::AppError;

use crate::action::environment::Environment;
use crate::action::errors::{InvalidInput, InvalidOutput};
use crate::action::execute::git;
use crate::action::native::NativeHost;
use crate::action::port::{Host, Output, Process};
use crate::action::preparation::inputs::WorkflowInputs;
use crate::action::preparation::scope::Workspace;
use crate::action::preparation::{Flow, PrepareWorkflowArgs};
use crate::model::CommitSha;
use crate::workflow::projection::{matrix_outputs, platform_outputs};

/// Prepares workflow execution without constructing credentials or a publication client.
// Native adapter selection has integration coverage; prepare_with owns fake-driven policy.
#[cfg_attr(test, mutants::skip)]
pub(crate) async fn prepare_workflow(args: PrepareWorkflowArgs) -> Result<(), AppError> {
    prepare_with(args, &NativeHost).await
}

/// Freezes event identity, canonical configuration and concrete scope before emitting outputs.
pub(crate) async fn prepare_with(
    args: PrepareWorkflowArgs,
    host: &impl Host,
) -> Result<(), AppError> {
    let invocation = host.current_dir()?;
    let inputs = WorkflowInputs::parse(
        &host.read(&rebase(&invocation, args.inputs_file))?,
        args.flow,
    )?;
    let cwd = host.directory(&inputs.get("working-directory").map_or_else(
        || invocation.clone(),
        |path| rebase(&invocation, path.into()),
    ))?;
    let output = host.output_file(&rebase(&cwd, args.github_output))?;
    let environment = Environment::read(host, &invocation)?;
    let instance = host
        .instance(&cwd, inputs.get("config").map(Path::new))
        .await?;
    let mut outputs = if args.flow == Flow::Backfill {
        platform_outputs(inputs.platforms(), &instance)?
    } else {
        matrix_outputs(inputs.platforms(), &instance)?
    };
    if environment.fork() {
        host.note(
            "Skipping fork-origin PR workflow preparation; this is not an empty benchmark scope.",
        );
        outputs.push_str("skipped=true\nskip-reason=fork-pull-request\n");
        if args.flow != Flow::Backfill {
            outputs.push_str("skip-all=true\npackages=\n");
        }
        return host.append_outputs(&output, &outputs);
    }
    if args.flow == Flow::Pr
        && (environment.value("GITHUB_EVENT_NAME") != Some("pull_request")
            || environment.pull_request().is_none())
    {
        return Err(InvalidInput::new(
            "flow",
            "PR preparation requires an ordinary pull_request event",
        )
        .into());
    }
    match git(host, &cwd, &["rev-parse", "--is-shallow-repository"])
        .await?
        .trim()
    {
        "false" => {}
        "true" => return Err(InvalidInput::new("checkout", "full Git history is required").into()),
        _ => return Err(InvalidOutput::new("invalid Git shallow status").into()),
    }
    let head = resolve(host, &cwd, "HEAD").await?;
    if let Some(expected) = environment.head()
        && head != expected.parse::<CommitSha>()?
    {
        return Err(
            InvalidInput::new("checkout", "HEAD differs from the frozen event commit").into(),
        );
    }
    let base = match args.flow {
        Flow::Backfill => {
            let from = resolve(
                host,
                &cwd,
                inputs
                    .get("from")
                    .expect("backfill input validation requires its range start"),
            )
            .await?;
            let to = resolve(
                host,
                &cwd,
                inputs
                    .get("to")
                    .expect("backfill input validation requires its range end"),
            )
            .await?;
            // Historical commits own their benchmark inventories, not this invocation's HEAD.
            // The core backfill command owns first-parent range validation and traversal.
            host.note(&format!(
                "Prepared backfill for {} from {} to {} using invocation head {}. Historical workspaces determine benchmark scope; exclusions={:?}.",
                instance.as_str(), from.as_str(), to.as_str(), head.as_str(), inputs.excluded,
            ));
            writeln!(
                outputs,
                "from={}\nto={}\nskipped=false",
                from.as_str(),
                to.as_str()
            )
            .expect("formatting into a String cannot fail");
            return host.append_outputs(&output, &outputs);
        }
        Flow::History => head.clone(),
        Flow::Pr => {
            let base: CommitSha = environment
                .base()
                .ok_or_else(|| InvalidInput::new("event", "PR base commit is required"))?
                .parse()?;
            let resolved = resolve(host, &cwd, base.as_str()).await?;
            if resolved != base {
                return Err(InvalidInput::new("event", "PR base is not a commit").into());
            }
            base
        }
    };
    let metadata = host
        .process(&Process {
            program: "cargo".into(),
            args: [
                "metadata",
                "--format-version=1",
                "--no-deps",
                "--all-features",
                "--locked",
                "--offline",
            ]
            .into_iter()
            .map(Into::into)
            .collect(),
            cwd: cwd.clone(),
            output: Output::Capture,
        })
        .await?;
    let workspace = Workspace::parse(&metadata, host)?;
    let affected = if args.flow == Flow::Pr {
        affected_packages(host, &cwd, &workspace, &head, &base).await?
    } else {
        None
    };
    let packages = workspace.select(affected, &inputs.excluded)?;
    let packages = packages.into_iter().collect::<Vec<_>>().join(",");
    host.note(&format!(
        "Prepared {:?} at {} against {}; scope={}, exclusions={:?}, benchmark packages=[{}]. Dependency expansion precedes benchmark filtering and exclusions.",
        args.flow, head.as_str(), base.as_str(), if args.flow == Flow::Pr { "affected" } else { "workspace" },
        inputs.excluded, packages,
    ));
    writeln!(
        outputs,
        "head={}\nbase={}\npackages={packages}\nskip-all={}\nskipped=false",
        head.as_str(),
        base.as_str(),
        packages.is_empty(),
    )
    .expect("formatting into a String cannot fail");
    host.append_outputs(&output, &outputs)
}

/// Resolves an actual commit without fetching or accepting an option as a revision.
async fn resolve(host: &impl Host, cwd: &Path, reference: &str) -> Result<CommitSha, AppError> {
    git(
        host,
        cwd,
        &[
            "rev-parse",
            "--verify",
            "--end-of-options",
            &format!("{reference}^{{commit}}"),
        ],
    )
    .await?
    .trim()
    .parse()
}

/// Maps both sides of renames/deletions through the detector; workspace files select all members.
async fn affected_packages(
    host: &impl Host,
    cwd: &Path,
    workspace: &Workspace,
    head: &CommitSha,
    base: &CommitSha,
) -> Result<Option<BTreeSet<String>>, AppError> {
    let root = git(host, cwd, &["rev-parse", "--show-toplevel"]).await?;
    let root = host.directory(Path::new(root.trim()))?;
    let changed = git(
        host,
        &root,
        &[
            "diff",
            "--no-ext-diff",
            "--no-renames",
            "--name-only",
            "-z",
            &format!("{}...{}", base.as_str(), head.as_str()),
            "--",
        ],
    )
    .await?;
    if !changed.is_empty() && !changed.ends_with('\0') {
        return Err(InvalidOutput::new("Git changed paths must be NUL-terminated").into());
    }
    let mut owners = BTreeSet::new();
    for path in changed.split_terminator('\0') {
        if path.is_empty()
            || Path::new(path).is_absolute()
            || Path::new(path)
                .components()
                .any(|part| part == Component::ParentDir)
        {
            return Err(InvalidOutput::new("invalid repository-relative Git path").into());
        }
        let path = root.join(path);
        // Repository-wide inputs outside a nested Cargo workspace can affect all of it.
        // Ref: docs/design.md, Workflow preparation.
        if !path.starts_with(&workspace.root) {
            return Ok(None);
        }
        // A separate nested workspace is not part of this Cargo inventory. Resolve the
        // declared owner boundary so its test fixtures cannot become foreign package names.
        let Some(directory) = workspace.package_directory(&path) else {
            return Ok(None);
        };
        match host.package(&workspace.root, directory)? {
            Some(name) => {
                owners.insert(name);
            }
            None => return Ok(None),
        }
    }
    Ok(Some(owners))
}
