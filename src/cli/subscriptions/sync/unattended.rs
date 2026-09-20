//! What `brama subscription sync` needs to run as a service rather than as a
//! command somebody remembers to type.
//!
//! On 2026-09-20 every subscription credential the gateway held was dead, the
//! automatic sign-in could not repair any of them (Google wanted an
//! authenticator code the vault does not hold), and the fleet served nothing
//! for a day — while a live `claude-code` grant sat in omp's store on
//! `lukasz-macbook`, one `brama subscription sync` away. The sweep existed;
//! nothing ran it. A capability only an operator can trigger is not a repair.
//!
//! Two things stood between the sweep and a launchd job: the console bearer
//! arrived on stdin, and the gateway origin had to be typed as a URL whose
//! port the resolver assigns. Both are answered here by asking Stado, at every
//! pass, so a rotated token and a moved port are picked up without editing a
//! service declaration, and no secret is written to disk.

use std::time::Duration;

use zeroize::Zeroizing;

/// A lookup that has not answered in this long is not going to.
const LOOKUP_TIMEOUT: Duration = Duration::from_secs(30);

/// The Stado binary that answers both lookups, resolved exactly as the
/// readiness path resolves it.
fn stado_binary() -> std::path::PathBuf {
    std::env::var("BRAMA_STADO_BIN")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| {
            let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
            std::path::PathBuf::from(home)
                .join(".stado")
                .join("bin")
                .join("stado")
        })
}

async fn stado(arguments: &[&str]) -> Result<Zeroizing<String>, String> {
    let binary = stado_binary();
    let output = tokio::time::timeout(
        LOOKUP_TIMEOUT,
        tokio::process::Command::new(&binary)
            .kill_on_drop(true)
            .args(arguments)
            .output(),
    )
    .await
    .map_err(|_| {
        format!(
            "`{} {}` did not answer within {}s",
            binary.display(),
            arguments.join(" "),
            LOOKUP_TIMEOUT.as_secs()
        )
    })?
    .map_err(|error| format!("`{}` could not be run: {error}", binary.display()))?;
    if !output.status.success() {
        let said: String = String::from_utf8_lossy(&output.stderr)
            .trim()
            .chars()
            .take(300)
            .collect();
        return Err(format!(
            "`{} {}` refused: {said}",
            binary.display(),
            arguments.join(" ")
        ));
    }
    Ok(Zeroizing::new(
        String::from_utf8_lossy(&output.stdout).trim().to_string(),
    ))
}

/// One `<item>#<field>` coordinate, split exactly once.
pub(crate) fn split_item_field(coordinate: &str) -> Result<(&str, &str), String> {
    match coordinate.split_once('#') {
        Some((item, field)) if !item.trim().is_empty() && !field.trim().is_empty() => {
            Ok((item.trim(), field.trim()))
        }
        _ => Err(format!(
            "--bearer-item is `<item>#<field>`, and `{coordinate}` is not"
        )),
    }
}

/// The console bearer, read out of the vault at the moment it is used.
///
/// Read per pass rather than once at start: a service that cached it would
/// keep presenting a rotated token until somebody restarted it.
pub(crate) async fn bearer_from_vault(coordinate: &str) -> Result<Zeroizing<String>, String> {
    let (item, field) = split_item_field(coordinate)?;
    let bearer = stado(&["credentials", "get", item, "--field", field]).await?;
    if bearer.trim().is_empty() {
        return Err(format!("the vault holds no value at {item}#{field}"));
    }
    Ok(bearer)
}

/// The gateway origin Stado's service directory gives this consumer.
pub(crate) async fn gateway_for_consumer(consumer: &str) -> Result<String, String> {
    let answer = stado(&[
        "service",
        "directory",
        "connect",
        "brama",
        "--consumer",
        consumer,
    ])
    .await?;
    // The directory prints the origin first and its observation after it.
    let origin = answer
        .split_whitespace()
        .next()
        .unwrap_or_default()
        .to_string();
    if !origin.starts_with("http://") && !origin.starts_with("https://") {
        return Err(format!(
            "Stado's service directory answered `{}` for consumer `{consumer}`, which names no gateway origin",
            answer.as_str()
        ));
    }
    Ok(origin)
}

#[cfg(test)]
mod tests {
    use super::{gateway_for_consumer, split_item_field};

    #[test]
    fn an_item_field_coordinate_splits_once() {
        assert_eq!(
            split_item_field("brama-desktop-model-router#token"),
            Ok(("brama-desktop-model-router", "token"))
        );
    }

    #[test]
    fn a_coordinate_without_a_field_is_refused() {
        for bad in ["brama-desktop-model-router", "#token", "item#"] {
            assert!(split_item_field(bad).is_err(), "{bad}");
        }
    }

    /// A consumer the directory does not serve must not become a gateway URL
    /// the sweep then posts a bearer to.
    #[tokio::test]
    async fn a_directory_answer_that_names_no_origin_is_refused() {
        let error = gateway_for_consumer("no-such-consumer-for-this-test")
            .await
            .expect_err("an unknown consumer cannot resolve to an origin");
        assert!(
            !error.contains("http://"),
            "the refusal names no origin: {error}"
        );
    }
}
