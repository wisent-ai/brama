//! `brama adopt`: reviewing an existing Brama route registry, and persisting
//! the aliases the operator named from it.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use clap::Args;

mod render;

use render::{print_adoption_preview, print_adoption_result};

#[derive(Args)]
pub(crate) struct AdoptArgs {
    /// Existing Brama inference-routes JSON file to review
    #[arg(long, value_name = "FILE")]
    from: PathBuf,
    /// Destination route registry; defaults to BRAMA_INFERENCE_ROUTES_FILE or ~/.config/brama/inference-routes.json
    #[arg(long, value_name = "FILE")]
    into: Option<PathBuf>,
    /// Agent whose Skarbiec subscription identities should be discovered
    #[arg(long, default_value = "wisent-app")]
    agent_id: String,
    /// Persist the selected aliases after showing the same review data
    #[arg(long, default_value_t = false)]
    apply: bool,
    /// Exact source alias to persist; repeat for more than one
    #[arg(long = "select", value_name = "ALIAS")]
    selected_aliases: Vec<String>,
    /// Select every importable or already unchanged alias
    #[arg(long, default_value_t = false)]
    all_importable: bool,
    /// Replace conflicting aliases; deployment conflicts are never replaced
    #[arg(long, default_value_t = false)]
    replace_conflicts: bool,
    /// Print the preview or result as JSON
    #[arg(long, default_value_t = false)]
    json: bool,
}

/// What the operator selected for adoption: the switch that persists anything,
/// and the three ways of naming which aliases.
pub(in crate::cli) struct AdoptionSelection<'a> {
    pub(in crate::cli) apply: bool,
    pub(in crate::cli) selected_aliases: &'a [String],
    pub(in crate::cli) all_importable: bool,
    pub(in crate::cli) replace_conflicts: bool,
}

pub(crate) async fn adopt(args: AdoptArgs) {
    let AdoptArgs {
        from,
        into,
        agent_id,
        apply,
        selected_aliases,
        all_importable,
        replace_conflicts,
        json,
    } = args;
    if let Err(error) = run_adoption(
        &from,
        into.as_deref(),
        &agent_id,
        AdoptionSelection {
            apply,
            selected_aliases: &selected_aliases,
            all_importable,
            replace_conflicts,
        },
        json,
    )
    .await
    {
        eprintln!("Configuration adoption error: {error}");
        std::process::exit(1);
    }
}

pub(in crate::cli) async fn run_adoption(
    from: &Path,
    into: Option<&Path>,
    agent_id: &str,
    selection: AdoptionSelection<'_>,
    json: bool,
) -> Result<bool, String> {
    let AdoptionSelection {
        apply,
        selected_aliases,
        all_importable,
        replace_conflicts,
    } = selection;
    if !apply && (all_importable || replace_conflicts || !selected_aliases.is_empty()) {
        return Err(
            "--apply is required with --select, --all-importable, or --replace-conflicts"
                .to_string(),
        );
    }
    if apply && !all_importable && selected_aliases.is_empty() {
        return Err(
            "--apply requires at least one --select <ALIAS> or --all-importable".to_string(),
        );
    }
    if selected_aliases.iter().collect::<HashSet<_>>().len() != selected_aliases.len() {
        return Err("selected aliases must be unique".to_string());
    }
    let document = read_adoption_source(from)?;
    let destination = match into {
        Some(path) => path.to_path_buf(),
        None => brama::config_adoption::default_destination()?,
    };
    let source_name = from.display().to_string();
    let preview =
        brama::config_adoption::preview_document(&document, &source_name, &destination, agent_id)
            .await?;
    if !apply {
        if json {
            println!(
                "{}",
                serde_json::to_string_pretty(&preview)
                    .map_err(|error| format!("cannot encode adoption preview: {error}"))?
            );
        } else {
            print_adoption_preview(&preview);
        }
        return Ok(false);
    }

    let mut selection = selected_aliases.to_vec();
    if all_importable {
        selection.extend(
            preview
                .candidates
                .iter()
                .filter(|candidate| {
                    matches!(
                        candidate.disposition,
                        brama::config_adoption::AdoptionDisposition::Importable
                            | brama::config_adoption::AdoptionDisposition::Unchanged
                    )
                })
                .map(|candidate| candidate.alias.clone()),
        );
    }
    selection.sort();
    selection.dedup();
    let result = brama::config_adoption::apply_document(
        &document,
        &source_name,
        &destination,
        agent_id,
        &selection,
        replace_conflicts,
    )
    .await?;
    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&result)
                .map_err(|error| format!("cannot encode adoption result: {error}"))?
        );
    } else {
        print_adoption_result(&result);
    }
    Ok(true)
}

fn read_adoption_source(path: &Path) -> Result<String, String> {
    let metadata = std::fs::symlink_metadata(path)
        .map_err(|error| format!("cannot inspect {}: {error}", path.display()))?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err(format!(
            "{} must be a regular non-symlink file",
            path.display()
        ));
    }
    if metadata.len() > 1024 * 1024 {
        return Err(format!(
            "{} exceeds the 1048576-byte configuration limit",
            path.display()
        ));
    }
    std::fs::read_to_string(path)
        .map_err(|error| format!("cannot read {} as UTF-8: {error}", path.display()))
}
