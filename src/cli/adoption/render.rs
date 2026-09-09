//! What a registry review and a persisted registry import read like as lines.

pub(super) fn print_adoption_preview(preview: &brama::config_adoption::AdoptionPreview) {
    println!("Source: {}", preview.source);
    println!("Destination: {}", preview.destination);
    println!("Agent: {}", preview.agent_id);
    println!(
        "Configured provider acquisitions: {}",
        preview.providers.len()
    );
    for provider in &preview.providers {
        println!("  {} ({})", provider.provider, provider.acquisition);
    }
    println!(
        "Skarbiec subscriptions: {} ({})",
        preview.subscriptions.len(),
        preview.subscription_discovery
    );
    for subscription in &preview.subscriptions {
        println!(
            "  {} / {} ({})",
            subscription.provider, subscription.subscription_id, subscription.status
        );
    }
    println!("Route candidates: {}", preview.candidates.len());
    for candidate in &preview.candidates {
        println!(
            "  {:<11} {} = {}",
            adoption_disposition(candidate.disposition),
            candidate.alias,
            candidate.primary
        );
        println!("      {}", candidate.detail);
        if let Some(primary) = candidate.existing_primary.as_deref() {
            println!("      current: {primary}");
        }
        if !candidate.deployments.is_empty() {
            println!("      deployments: {}", candidate.deployments.join(", "));
        }
    }
    for deployment in &preview.unreferenced_deployments {
        println!("  rejected    deployment {deployment}: no source alias references it");
    }
}

pub(super) fn print_adoption_result(result: &brama::config_adoption::AdoptionResult) {
    println!("Destination: {}", result.destination);
    println!(
        "Imported: {}; unchanged: {}; conflicting: {}; rejected: {}",
        result.imported, result.unchanged, result.conflicting, result.rejected
    );
    for item in &result.items {
        println!(
            "  {:<11} {}: {}",
            adoption_disposition(item.disposition),
            item.alias,
            item.detail
        );
    }
}

fn adoption_disposition(disposition: brama::config_adoption::AdoptionDisposition) -> &'static str {
    use brama::config_adoption::AdoptionDisposition;
    match disposition {
        AdoptionDisposition::Importable => "importable",
        AdoptionDisposition::Imported => "imported",
        AdoptionDisposition::Unchanged => "unchanged",
        AdoptionDisposition::Conflicting => "conflicting",
        AdoptionDisposition::Rejected => "rejected",
    }
}
