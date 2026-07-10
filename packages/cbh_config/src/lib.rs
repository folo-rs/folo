#![cfg_attr(coverage_nightly, feature(coverage_attribute))]
#![cfg_attr(docsrs, feature(doc_cfg))]
#![doc(hidden)]
#![expect(
    clippy::exhaustive_enums,
    clippy::exhaustive_structs,
    reason = "this crate's `pub` items form an in-workspace handoff boundary to the \
              cargo-bench-history shell crate, used only inside this workspace rather \
              than as a stable public API. Exhaustive construction and matching by \
              those in-workspace consumers is intended"
)]

//! Implementation crate for [`cargo-bench-history`]; do not depend on this directly.
//!
//! The configuration loaded from `.cargo/bench_history.toml`: which project this is
//! and where its benchmark history is stored. Carries the parsed [`Config`] model, the
//! TOML [`parse_config`]/[`load_config`] entry points, the starter
//! [`default_template`], the [`ConfigError`] type, and the command *input resolution*
//! (locating the configuration file and repository, reading the storage/cache
//! environment variables, and turning the parsed `--local`/`--cache` selections into
//! concrete paths). Split out of the `cargo-bench-history` shell so this config
//! parsing is cheap to mutation-test in isolation.
//!
//! Every item is re-exported flat from the crate root, so consumers write
//! `cbh_config::Config` rather than reaching into a submodule.
//!
//! [`cargo-bench-history`]: https://github.com/folo-rs/folo

mod config;
mod input;

pub use config::{
    AzureStorageConfig, CloudStorageConfig, Config, ConfigError, ProjectConfig, default_template,
    load_config, parse_config,
};
pub use input::{
    CACHE_ENV_VAR, STORAGE_ENV_VAR, cache_env, default_config_path, rebase, resolve_cache_path,
    resolve_config_path, resolve_local_path, resolve_project_id, resolve_repo, storage_env,
};
