//! `cargo xtask`: the command line over the library. Each subcommand prints the summary
//! line its library function returns and exits non-zero on error.

use std::path::PathBuf;
use std::process::ExitCode;

use anyhow::Result;
use clap::{ArgAction, ArgGroup, Args, Parser, Subcommand};
use xtask::platform_assets::Platform;
use xtask::upstream::{self, update::UpdateOptions};
use xtask::{
    audit, catalog, changelog, check, ci, efficiency, llms, local_assets, meta, platform_assets,
    release_assets, release_contract, repo_root, sbom,
};

#[derive(Parser)]
#[command(
    name = "xtask",
    about = "Pulse repository checks, release steps and hygiene commands"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Run the full local gate: formatting, lint, types, tests, changelog, llms.txt, catalog
    Check {
        /// Skip a step by id (repeatable): fmt, bun-fmt, lint, typecheck, clippy, bun-test,
        /// nextest, changelog, llms, catalog-parity
        #[arg(long = "skip", value_name = "STEP")]
        skip: Vec<String>,
    },
    /// Write llms.txt and llms-full.txt from docs/llms.toml
    Llms {
        /// Fail instead of writing when either file is out of date
        #[arg(long)]
        check: bool,
    },
    /// Check CHANGELOG.md or print a version's release notes
    Changelog {
        #[command(subcommand)]
        action: ChangelogAction,
    },
    /// Compare or apply the GitHub description, homepage and topics
    Meta(MetaArgs),
    /// Check the Codex model catalog against the canonical copy and the pinned rates
    CatalogParity {
        /// Checkout of Codex-Discord-Rich-Presence (default: the sibling folder)
        #[arg(long)]
        upstream_root: Option<PathBuf>,
    },
    /// Verify or refresh the vendored Codex runtime
    Upstream {
        #[command(subcommand)]
        action: UpstreamAction,
    },
    /// Verify a release tag, every version surface and the release contract
    ReleaseContract {
        /// Release tag to verify, for example v1.9.7
        #[arg(long)]
        tag: String,
        /// Commit the tag must point to
        #[arg(long)]
        expected_commit: String,
        /// Repository root to verify (default: this checkout)
        #[arg(long)]
        root: Option<PathBuf>,
        /// Fixture mode: do not require HEAD to be the tagged commit
        #[arg(long)]
        test_mode: bool,
    },
    /// Assemble the public release assets, latest.json and SHA256SUMS.txt
    ReleaseAssets {
        /// Folder holding the downloaded per-platform build artifacts
        #[arg(long)]
        artifacts: PathBuf,
        /// Folder to write the public release assets into
        #[arg(long)]
        output: PathBuf,
        /// Release tag, for example v1.9.7
        #[arg(long)]
        tag: String,
        /// GitHub repository as owner/name, used for the download URLs in latest.json
        #[arg(long)]
        repository: String,
    },
    /// Collect one platform's installers under platform-prefixed names
    PlatformAssets {
        /// Bundle folders to try, first existing one wins (repeatable)
        #[arg(long, required = true)]
        input: Vec<PathBuf>,
        /// Folder to write the platform-prefixed installers into
        #[arg(long)]
        output: PathBuf,
        /// Platform whose installers are collected
        #[arg(long, value_enum)]
        platform: Platform,
        /// Unsigned native verification: drop the signature requirements
        #[arg(long, action = ArgAction::Set, default_value_t = false, value_name = "BOOL")]
        verification_only: bool,
    },
    /// Prepare the Windows x64 recovery release folder
    LocalReleaseAssets {
        /// Tauri bundle folder of the Windows x64 build
        #[arg(long)]
        bundle: PathBuf,
        /// Release folder to create; it must not exist yet
        #[arg(long)]
        output: PathBuf,
        /// Release tag, for example v1.9.7
        #[arg(long)]
        tag: String,
    },
    /// Run cargo audit with the reviewed RustSec exceptions
    Audit,
    /// Windows SPDX SBOM generation and validation
    Sbom {
        #[command(subcommand)]
        action: SbomAction,
    },
    /// Read a running process's EcoQoS and priority state (Windows only)
    WindowsEfficiency {
        /// Id of the process to inspect
        #[arg(long)]
        pid: u32,
    },
    /// Write the Tauri bundle arguments of a release build job
    BundleArgs {
        /// Rust target triple of the build job
        #[arg(long)]
        target: String,
        /// Tauri bundle formats passed to `--bundles`
        #[arg(long)]
        bundles: String,
        /// Whether the run publishes the release; when false, updater artifacts are turned off
        #[arg(long, action = ArgAction::Set, default_value_t = false, value_name = "BOOL")]
        publish: bool,
    },
    /// Prove the runner is native for a target, then run Clippy and the tests for it
    VerifyNative {
        /// Rust target triple the runner must execute natively
        #[arg(long)]
        target: String,
    },
}

#[derive(Subcommand)]
enum ChangelogAction {
    /// Check CHANGELOG.md against the Keep a Changelog 2.0.0 rules
    Check,
    /// Print the release notes of a version
    Notes {
        /// Version whose notes are printed, as written in the changelog heading
        version: String,
        /// Write to a file instead of stdout
        #[arg(long)]
        output: Option<PathBuf>,
    },
}

#[derive(Args)]
#[command(group(ArgGroup::new("mode").required(true).multiple(false).args(["check", "apply"])))]
struct MetaArgs {
    /// Fail when GitHub differs from .github/repository.toml
    #[arg(long)]
    check: bool,
    /// Edit the GitHub repository to match .github/repository.toml
    #[arg(long)]
    apply: bool,
}

#[derive(Subcommand)]
enum UpstreamAction {
    /// Verify src/codex/UPSTREAM.json
    Check {
        /// Repository root to verify (default: this checkout)
        #[arg(long)]
        root: Option<PathBuf>,
        /// Fixture mode: accept inventories whose provenance is "test"
        #[arg(long)]
        test_mode: bool,
    },
    /// Open, update or close the drift issue when the core pin trails upstream main (needs gh)
    Freshness,
    /// Vendor the Codex runtime from an annotated upstream tag
    Update {
        /// Annotated upstream tag to vendor
        #[arg(long)]
        tag: String,
        /// Commit the tag must resolve to
        #[arg(long)]
        commit: String,
        /// Repository root to update (default: this checkout)
        #[arg(long)]
        root: Option<PathBuf>,
        /// Fixture mode: vendor from --test-repository instead of the remote
        #[arg(long)]
        test_mode: bool,
        /// Local clone to sync from; only with --test-mode
        #[arg(long)]
        test_repository: Option<PathBuf>,
    },
}

#[derive(Subcommand)]
enum SbomAction {
    /// Generate the SBOM of an installer from cargo metadata
    New {
        /// Installer the SBOM describes
        #[arg(long)]
        artifact: PathBuf,
        /// SBOM file to write
        #[arg(long)]
        output: PathBuf,
        /// Package name recorded in the SBOM
        #[arg(long)]
        package_name: String,
        /// Package version recorded in the SBOM
        #[arg(long)]
        package_version: String,
        /// Repository root whose dependency graph is used (default: this checkout)
        #[arg(long)]
        root: Option<PathBuf>,
    },
    /// Validate an SBOM against its installer
    Check {
        /// Installer the SBOM must describe
        #[arg(long)]
        artifact: PathBuf,
        /// SBOM file to validate
        #[arg(long)]
        sbom: PathBuf,
        /// Package name the SBOM must record
        #[arg(long)]
        package_name: String,
        /// Package version the SBOM must record
        #[arg(long)]
        package_version: String,
    },
    /// Find the one .exe in a folder, then generate and validate its SBOM
    Ci {
        /// Folder that holds exactly one installer .exe
        #[arg(long)]
        dist: PathBuf,
        /// Platform used in the SBOM file name
        #[arg(long, value_enum)]
        platform: Platform,
    },
}

fn root_or_default(root: Option<PathBuf>) -> PathBuf {
    root.unwrap_or_else(repo_root)
}

fn run(cli: Cli) -> Result<()> {
    let default_root = repo_root();
    let contract = default_root.join(upstream::CONTRACT_RELATIVE);
    let message = match cli.command {
        Command::Check { skip } => {
            check::run(&default_root, &skip)?;
            return Ok(());
        }
        Command::Llms { check } => llms::run(&default_root, check)?,
        Command::Changelog { action } => match action {
            ChangelogAction::Check => changelog::check(&default_root)?,
            ChangelogAction::Notes { version, output } => {
                let notes = changelog::notes(&default_root, &version)?;
                match output {
                    Some(path) => {
                        xtask::fsx::write_text(&path, &notes)?;
                        format!("Release notes for {version} written to {}", path.display())
                    }
                    None => {
                        print!("{notes}");
                        return Ok(());
                    }
                }
            }
        },
        Command::Meta(args) => meta::run(&default_root, args.apply)?,
        Command::CatalogParity { upstream_root } => {
            catalog::parity(&default_root, upstream_root.as_deref())?
        }
        Command::Upstream { action } => match action {
            UpstreamAction::Check { root, test_mode } => {
                upstream::check::run(&root_or_default(root), &contract, test_mode)?
            }
            UpstreamAction::Freshness => upstream::freshness::run(&default_root)?,
            UpstreamAction::Update {
                tag,
                commit,
                root,
                test_mode,
                test_repository,
            } => upstream::update::run(&UpdateOptions {
                root: &root_or_default(root),
                contract_path: &contract,
                tag: &tag,
                commit: &commit,
                test_repository: test_repository.as_deref(),
                test_mode,
            })?,
        },
        Command::ReleaseContract {
            tag,
            expected_commit,
            root,
            test_mode,
        } => release_contract::check(&root_or_default(root), &tag, &expected_commit, test_mode)?,
        Command::ReleaseAssets {
            artifacts,
            output,
            tag,
            repository,
        } => release_assets::assemble(&artifacts, &output, &tag, &repository)?,
        Command::PlatformAssets {
            input,
            output,
            platform,
            verification_only,
        } => platform_assets::collect_first_existing(&input, &output, platform, verification_only)?,
        Command::LocalReleaseAssets {
            bundle,
            output,
            tag,
        } => local_assets::prepare(&bundle, &output, &tag)?,
        Command::Audit => audit::run(&default_root)?,
        Command::Sbom { action } => match action {
            SbomAction::New {
                artifact,
                output,
                package_name,
                package_version,
                root,
            } => sbom::generate(
                &artifact,
                &output,
                &package_name,
                &package_version,
                &root_or_default(root),
            )?,
            SbomAction::Check {
                artifact,
                sbom,
                package_name,
                package_version,
            } => sbom::verify(&artifact, &sbom, &package_name, &package_version)?,
            SbomAction::Ci { dist, platform } => {
                sbom::generate_and_verify(&dist, platform, &default_root)?
            }
        },
        Command::WindowsEfficiency { pid } => efficiency::probe(pid)?,
        Command::BundleArgs {
            target,
            bundles,
            publish,
        } => {
            let line = ci::bundle_args(&default_root, &target, &bundles, publish)?;
            ci::write_github_output(&line)?;
            return Ok(());
        }
        Command::VerifyNative { target } => ci::verify_native(&default_root, &target)?,
    };
    println!("{message}");
    Ok(())
}

fn main() -> ExitCode {
    match run(Cli::parse()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("error: {error:#}");
            ExitCode::FAILURE
        }
    }
}
