//! `polkagent package` — durable local plugin and product-kit lifecycle.

use std::io::{self, IsTerminal as _, Write as _};
use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result};
use polkagent_marketplace::{
    inspect_local_package, InstallOutcome, InstalledPackage, LocalInstallPolicy, LocalPackageError,
    LocalPackageStore, PackageCandidate, PackageKind, ProvenanceStatus, RollbackOutcome,
    UninstallOutcome, UpdateOutcome,
};
use serde_json::{json, Value};

use crate::cli::{
    PackageArgs, PackageCmd, PackageNameCmd, PackagePathCmd, PackageRollbackCmd, PackageTrustPolicy,
};
use crate::output::{format_output, OutputFormat};

/// Dispatch a local package lifecycle command.
pub fn run(
    args: &PackageArgs,
    format: OutputFormat,
    config_path: Option<&Path>,
    dry_run: bool,
    assume_yes: bool,
) -> Result<()> {
    let root = resolve_store_root(args.store.as_deref(), config_path)?;
    let policy = trust_policy(args.trust_policy);
    match &args.command {
        PackageCmd::Install(command) => {
            install(command, &root, policy, args.trust_policy, format, dry_run)
        }
        PackageCmd::List => list(&root, policy, format),
        PackageCmd::Get(command) => get(command, &root, policy, format),
        PackageCmd::Update(command) => {
            update(command, &root, policy, args.trust_policy, format, dry_run)
        }
        PackageCmd::Rollback(command) => rollback(command, &root, policy, format, dry_run),
        PackageCmd::Uninstall(command) => {
            uninstall(command, &root, policy, format, dry_run, assume_yes)
        }
    }
}

fn install(
    command: &PackagePathCmd,
    root: &Path,
    policy: LocalInstallPolicy,
    selected_policy: PackageTrustPolicy,
    format: OutputFormat,
    dry_run: bool,
) -> Result<()> {
    if dry_run {
        let candidate = inspect_and_validate(&command.path, policy)?;
        let existing = match existing_store(root, policy)? {
            Some(store) => store.get(&candidate.name).map_err(anyhow::Error::new)?,
            None => None,
        };
        let status = match existing {
            Some(existing)
                if existing.active.version == candidate.version
                    && existing.active.provenance.content_digest
                        == candidate.provenance.content_digest =>
            {
                "already_installed"
            }
            Some(existing) => {
                anyhow::bail!(
                    "package '{}' is already installed at version {}; use `polkagent package update`",
                    existing.name,
                    existing.active.version
                );
            }
            None => "would_install",
        };
        render_candidate("install", status, root, selected_policy, &candidate, format);
        return Ok(());
    }

    // Fail trust checks before initializing an empty store. The store repeats
    // validation under its mutation lock to close source-change races.
    inspect_and_validate(&command.path, policy)?;
    let store = open_store(root, policy)?;
    let outcome = store.install(&command.path).map_err(explain_trust_error)?;
    let (status, package) = match outcome {
        InstallOutcome::Installed(package) => ("installed", package),
        InstallOutcome::AlreadyInstalled(package) => ("already_installed", package),
    };
    render_package(
        "install",
        status,
        root,
        Some(selected_policy),
        &package,
        format,
    );
    Ok(())
}

fn list(root: &Path, policy: LocalInstallPolicy, format: OutputFormat) -> Result<()> {
    let packages = match existing_store(root, policy)? {
        Some(store) => store.list().map_err(anyhow::Error::new)?,
        None => Vec::new(),
    };
    if is_json(format) {
        emit(
            &json!({
                "ok": true,
                "operation": "list",
                "store": root,
                "count": packages.len(),
                "packages": packages,
            }),
            format,
        );
        return Ok(());
    }

    if packages.is_empty() {
        println!("No local packages installed in {}.", root.display());
        return Ok(());
    }
    println!(
        "{:<32}  {:<12}  {:<8}  History  Provenance",
        "Name", "Version", "Kind"
    );
    println!("{}", "-".repeat(86));
    for package in &packages {
        println!(
            "{:<32}  {:<12}  {:<8}  {:<7}  {}",
            package.name,
            package.active.version,
            kind_name(package.kind),
            package.history.len(),
            provenance_name(package.active.provenance.status),
        );
    }
    Ok(())
}

fn get(
    command: &PackageNameCmd,
    root: &Path,
    policy: LocalInstallPolicy,
    format: OutputFormat,
) -> Result<()> {
    let package = find_package(root, policy, &command.name)?;
    if is_json(format) {
        emit(
            &json!({
                "ok": true,
                "operation": "get",
                "store": root,
                "package": package,
            }),
            format,
        );
        return Ok(());
    }
    print_package(&package, root);
    Ok(())
}

fn update(
    command: &PackagePathCmd,
    root: &Path,
    policy: LocalInstallPolicy,
    selected_policy: PackageTrustPolicy,
    format: OutputFormat,
    dry_run: bool,
) -> Result<()> {
    if dry_run {
        let candidate = inspect_and_validate(&command.path, policy)?;
        let existing = find_package(root, policy, &candidate.name)?;
        if existing.kind != candidate.kind {
            anyhow::bail!("package '{}' changes package kind", candidate.name);
        }
        let status = match existing.active.version.cmp(&candidate.version) {
            std::cmp::Ordering::Equal => {
                if existing.active.provenance.content_digest != candidate.provenance.content_digest
                {
                    anyhow::bail!(
                        "package '{}@{}' already exists with different content",
                        candidate.name,
                        candidate.version
                    );
                }
                "already_current"
            }
            std::cmp::Ordering::Greater => {
                anyhow::bail!(
                    "update for '{}' must be newer than {}, got {}",
                    candidate.name,
                    existing.active.version,
                    candidate.version
                );
            }
            std::cmp::Ordering::Less => "would_update",
        };
        render_candidate("update", status, root, selected_policy, &candidate, format);
        return Ok(());
    }

    inspect_and_validate(&command.path, policy)?;
    let store = open_store(root, policy)?;
    let outcome = store.update(&command.path).map_err(explain_trust_error)?;
    let (status, package) = match outcome {
        UpdateOutcome::Updated(package) => ("updated", package),
        UpdateOutcome::AlreadyCurrent(package) => ("already_current", package),
    };
    render_package(
        "update",
        status,
        root,
        Some(selected_policy),
        &package,
        format,
    );
    Ok(())
}

fn rollback(
    command: &PackageRollbackCmd,
    root: &Path,
    policy: LocalInstallPolicy,
    format: OutputFormat,
    dry_run: bool,
) -> Result<()> {
    if dry_run {
        let package = find_package(root, policy, &command.name)?;
        let target = match &command.to {
            Some(version) => package
                .history
                .iter()
                .find(|item| &item.version == version)
                .with_context(|| {
                    format!(
                        "version {version} is not retained for package '{}'",
                        command.name
                    )
                })?,
            None => package
                .history
                .last()
                .with_context(|| format!("package '{}' has no rollback history", command.name))?,
        };
        let value = json!({
            "ok": true,
            "operation": "rollback",
            "status": "would_rollback",
            "store": root,
            "name": package.name,
            "current_version": package.active.version,
            "target_version": target.version,
        });
        if is_json(format) {
            emit(&value, format);
            return Ok(());
        }
        println!(
            "Would roll back {} from {} to {}.",
            package.name, package.active.version, target.version
        );
        return Ok(());
    }

    let store = open_store(root, policy)?;
    let RollbackOutcome::RolledBack(package) = store
        .rollback(&command.name, command.to.as_ref())
        .map_err(anyhow::Error::new)?;
    render_package("rollback", "rolled_back", root, None, &package, format);
    Ok(())
}

fn uninstall(
    command: &PackageNameCmd,
    root: &Path,
    policy: LocalInstallPolicy,
    format: OutputFormat,
    dry_run: bool,
    assume_yes: bool,
) -> Result<()> {
    if dry_run {
        let package = find_package(root, policy, &command.name)?;
        if is_json(format) {
            emit(
                &json!({
                    "ok": true,
                    "operation": "uninstall",
                    "status": "would_uninstall",
                    "store": root,
                    "package": package,
                }),
                format,
            );
            return Ok(());
        }
        println!(
            "Would uninstall {}@{} from {}.",
            package.name,
            package.active.version,
            root.display()
        );
        return Ok(());
    }

    let package = find_package(root, policy, &command.name)?;
    if !assume_yes {
        if !io::stdin().is_terminal() {
            anyhow::bail!(
                "uninstall requires confirmation; pass --yes in non-interactive environments"
            );
        }
        print!(
            "Uninstall {}@{} and all retained versions? [y/N] ",
            package.name, package.active.version
        );
        io::stdout().flush()?;
        let mut answer = String::new();
        io::stdin().read_line(&mut answer)?;
        if !answer.trim().eq_ignore_ascii_case("y") {
            println!("Aborted.");
            return Ok(());
        }
    }

    let store = open_store(root, policy)?;
    let outcome = store.uninstall(&command.name).map_err(anyhow::Error::new)?;
    let status = match outcome {
        UninstallOutcome::Uninstalled => "uninstalled",
        UninstallOutcome::NotInstalled => "not_installed",
    };
    if is_json(format) {
        emit(
            &json!({
                "ok": true,
                "operation": "uninstall",
                "status": status,
                "store": root,
                "name": command.name,
            }),
            format,
        );
        return Ok(());
    }
    println!(
        "Package '{}' uninstalled from {}.",
        command.name,
        root.display()
    );
    Ok(())
}

fn render_candidate(
    operation: &str,
    status: &str,
    root: &Path,
    policy: PackageTrustPolicy,
    candidate: &PackageCandidate,
    format: OutputFormat,
) {
    if is_json(format) {
        emit(
            &json!({
                "ok": true,
                "operation": operation,
                "status": status,
                "store": root,
                "trust_policy": trust_policy_name(policy),
                "candidate": candidate,
            }),
            format,
        );
        return;
    }
    println!(
        "{}: {}@{} ({})",
        status.replace('_', " "),
        candidate.name,
        candidate.version,
        kind_name(candidate.kind)
    );
    println!("  Store:      {}", root.display());
    println!("  Digest:     {}", candidate.provenance.content_digest);
    print_development_warning(policy);
}

fn render_package(
    operation: &str,
    status: &str,
    root: &Path,
    policy: Option<PackageTrustPolicy>,
    package: &InstalledPackage,
    format: OutputFormat,
) {
    if is_json(format) {
        let mut value = json!({
            "ok": true,
            "operation": operation,
            "status": status,
            "store": root,
            "package": package,
        });
        if let Some(policy) = policy {
            value["trust_policy"] = Value::String(trust_policy_name(policy).to_owned());
        }
        emit(&value, format);
        return;
    }
    println!(
        "Package {status}: {}@{}",
        package.name, package.active.version
    );
    print_package(package, root);
    if let Some(policy) = policy {
        print_development_warning(policy);
    }
}

fn print_package(package: &InstalledPackage, root: &Path) {
    println!("  Kind:       {}", kind_name(package.kind));
    println!("  Store:      {}", root.display());
    println!("  Content:    {}", package.active.content_path.display());
    println!("  Digest:     {}", package.active.provenance.content_digest);
    println!(
        "  Provenance: {} (cryptographically verified: {})",
        provenance_name(package.active.provenance.status),
        package
            .active
            .provenance
            .signature_cryptographically_verified
    );
    if package.history.is_empty() {
        println!("  History:    none");
    } else {
        let versions = package
            .history
            .iter()
            .map(|item| item.version.to_string())
            .collect::<Vec<_>>()
            .join(", ");
        println!("  History:    {versions}");
    }
}

fn inspect_and_validate(path: &Path, policy: LocalInstallPolicy) -> Result<PackageCandidate> {
    let candidate = inspect_local_package(path).map_err(anyhow::Error::new)?;
    policy.validate(&candidate).map_err(explain_trust_error)?;
    Ok(candidate)
}

fn find_package(root: &Path, policy: LocalInstallPolicy, name: &str) -> Result<InstalledPackage> {
    let store = existing_store(root, policy)?
        .with_context(|| format!("package store '{}' is not initialized", root.display()))?;
    store
        .get(name)
        .map_err(anyhow::Error::new)?
        .with_context(|| format!("package '{name}' is not installed"))
}

fn open_store(root: &Path, policy: LocalInstallPolicy) -> Result<LocalPackageStore> {
    LocalPackageStore::open(root, policy)
        .map_err(anyhow::Error::new)
        .with_context(|| format!("opening local package store at '{}'", root.display()))
}

fn existing_store(root: &Path, policy: LocalInstallPolicy) -> Result<Option<LocalPackageStore>> {
    if !root.exists() {
        return Ok(None);
    }
    LocalPackageStore::open_existing(root, policy)
        .map(Some)
        .map_err(anyhow::Error::new)
        .with_context(|| format!("opening local package store at '{}'", root.display()))
}

fn trust_policy(policy: PackageTrustPolicy) -> LocalInstallPolicy {
    match policy {
        PackageTrustPolicy::Strict => LocalInstallPolicy::strict(),
        PackageTrustPolicy::Development => LocalInstallPolicy::development(),
    }
}

fn explain_trust_error(error: LocalPackageError) -> anyhow::Error {
    match error {
        LocalPackageError::UnsignedRejected(_)
        | LocalPackageError::UnverifiedSignatureRejected(_)
        | LocalPackageError::DigestRequired(_) => anyhow::anyhow!(
            "{error}. Strict trust is the default and fails closed because cryptographic package verification is not connected. For a local source you control, retry with `--trust-policy development` to acknowledge unsigned or unverified provenance; content integrity is still calculated and checked"
        ),
        other => anyhow::Error::new(other),
    }
}

fn resolve_store_root(explicit: Option<&Path>, config_path: Option<&Path>) -> Result<PathBuf> {
    if let Some(path) = explicit {
        return Ok(expand_home(path));
    }
    if let Some(path) =
        std::env::var_os("POLKAGENT_PACKAGE_STORE").filter(|value| !value.is_empty())
    {
        return Ok(expand_home(Path::new(&path)));
    }
    if let Some(config) = config_path {
        if let Some(parent) = config.parent() {
            return Ok(parent.join("packages"));
        }
    }
    if let Some(config) = polkagent_config::loader::find_project_config() {
        if let Some(parent) = config.parent() {
            return Ok(parent.join("packages"));
        }
    }
    let home = std::env::var_os("HOME")
        .filter(|value| !value.is_empty())
        .context("cannot resolve package store: HOME is unset; pass --store")?;
    Ok(PathBuf::from(home)
        .join(".local")
        .join("share")
        .join("polkagent")
        .join("packages"))
}

fn expand_home(path: &Path) -> PathBuf {
    let Some(text) = path.to_str() else {
        return path.to_path_buf();
    };
    if text == "~" {
        return std::env::var_os("HOME").map_or_else(|| path.to_path_buf(), PathBuf::from);
    }
    if let Some(remainder) = text.strip_prefix("~/") {
        if let Some(home) = std::env::var_os("HOME") {
            return PathBuf::from(home).join(remainder);
        }
    }
    path.to_path_buf()
}

fn emit(value: &Value, format: OutputFormat) {
    println!("{}", format_output(value, format, "package"));
}

fn is_json(format: OutputFormat) -> bool {
    matches!(format, OutputFormat::Json | OutputFormat::JsonPretty)
}

fn kind_name(kind: PackageKind) -> &'static str {
    match kind {
        PackageKind::Plugin => "plugin",
        PackageKind::Kit => "kit",
    }
}

fn provenance_name(status: ProvenanceStatus) -> &'static str {
    match status {
        ProvenanceStatus::Unsigned => "unsigned",
        ProvenanceStatus::SignatureClaimUnverified => "signature-claim-unverified",
    }
}

fn trust_policy_name(policy: PackageTrustPolicy) -> &'static str {
    match policy {
        PackageTrustPolicy::Strict => "strict",
        PackageTrustPolicy::Development => "development",
    }
}

fn print_development_warning(policy: PackageTrustPolicy) {
    if policy == PackageTrustPolicy::Development {
        eprintln!(
            "Warning: development trust policy accepted unsigned or cryptographically unverified local provenance."
        );
    }
}
