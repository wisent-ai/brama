/// So this one drives the built binary over a real Skarbiec vault and asks
/// for a route no account in the pool can pay for. Until 2026-09-16 that was
/// another agent's account; since every account serves every caller, it is
/// an account for another provider. Free to run: the roster is empty before
/// any provider is asked.
#[test]
fn the_dispatch_envelope_agrees_with_the_edge_about_an_absent_account() {
    let directory = TestDirectory::new("envelope-no-account");
    let vault = SkarbiecVault::create("envelope-no-account");
    vault.seed_subscription(
        "brama-envelope-owner",
        "claude-code",
        "envelope-owner-claude",
    );

    let mut command = Command::new(env!("CARGO_BIN_EXE_brama"));
    for (name, value) in vault.environment() {
        command.env(name, value);
    }
    let refused = command
        .env("HOME", directory.path().join("home"))
        .env("XDG_STATE_HOME", directory.path().join("xdg-state"))
        .env("BRAMA_STATE_DIR", directory.path().join("state"))
        .env(
            "BRAMA_SUBSCRIPTION_USAGE_FILE",
            directory.path().join("usage.json"),
        )
        .env("BRAMA_PERF_PATH", directory.path().join("perf.json"))
        .env("ENTITLEMENTS_ROUTER_BIN", vault.router())
        .args([
            "test",
            "--model",
            "codex/gpt-6-astra",
            "--agent-id",
            "brama-envelope-stranger",
            "--allow-provider-cost",
        ])
        .output()
        .expect("brama test for an agent with no account");

    let said = String::from_utf8_lossy(&refused.stderr);
    assert!(
        !refused.status.success(),
        "a call nothing can pay for must fail:\n{said}"
    );
    assert!(
        said.contains("no active 'codex' credential for agent"),
        "the refusal must name the provider and the agent:\n{said}"
    );
    assert!(
        said.contains(r#""failure_point":"brama.dispatch.credential-selection""#),
        "the envelope must say where the call broke:\n{said}"
    );
    assert!(
        said.contains(r#""error_code":"auth""#) && said.contains(r#""retryable":false"#),
        "an absent account is an authorization failure no wait repairs:\n{said}"
    );
    assert!(
        !said.contains(r#""error_code":"rate_limit""#),
        "the retry advice is back, and a caller will spend its attempts on it:\n{said}"
    );
}

/// A wait has to name the hour it ends.
///
/// The capacity sentence told the caller the quota "lifts on its own" and
/// sent them to `brama subscriptions`, which prints a block's reason and not
/// its end. On 2026-09-21 that left Oko's judge refused all day with no
/// instant anybody could name, while the ledger had held `blocked_until_ms`
/// since the block was recorded.
#[test]
fn a_capacity_refusal_names_the_instant_the_block_lifts() {
    let lifts_at = 1_790_000_000_000_i64; // 2026-09-21T14:13:20Z
    let named = capacity_summary("codex", true, Some(lifts_at));
    assert!(
        named.contains("the block lifts at 2026-09-21T14:13:20Z"),
        "the wait must name its end: {named}"
    );
    assert!(
        named.contains("need a sign-in"),
        "and still say what the other members need: {named}"
    );

    let unknown = capacity_summary("codex", false, None);
    assert!(
        !unknown.contains("the block lifts at"),
        "an instant the ledger does not hold is not invented: {unknown}"
    );
}
