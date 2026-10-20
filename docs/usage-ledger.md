# Subscription usage ledger

`$BRAMA_SUBSCRIPTION_USAGE_FILE`, by default
`~/.config/brama/subscription-usage.json` and readable only by its owner, holds
the per-subscription usage ledger: measured counters, the newest plan reading
per window with the instant it was read and where it came from, when the
provider's usage report was last checked, any block, and the newest check
verdict.

The token counters add only usage a provider stated. An answer whose provider
stated no usage adds nothing to them, adds one to `unmeasured_calls` and sets
`last_unmeasured_call_ms`, so a reader of the totals knows how many answers they
leave out. The ledger never counts such an answer as zero tokens.

It is written atomically, through a private staging file in the same directory
that is renamed over the ledger, so a reader never sees half a ledger and a
crash never truncates one. It is not a cache: the question it answers spans
months, not process lifetimes.

## Loading

- **No file:** a new, empty ledger.
- **A ledger written by an older gateway:** it loads. Readings that carry no
  observation instant, or the `0` an older format wrote for one not recorded,
  have no `recorded_at_ms`: they stay visible, the pool
  report shows their instant as not stated, and they are read as stale; the
  file's later modification time may be from another account and cannot prove
  when the provider measured this window. The usage-report verdict an older gateway kept in the completion
  probe's field is moved to its own field in memory only; a read never rewrites
  the file. `brama subscription list --refresh-usage` asks the provider for a
  new reading without discarding history if that request fails.
- **A file that exists but cannot be read or will not parse:** it is never
  replaced. The gateway keeps serving from memory, refuses every write to the
  file, and the subscription pool reports `brama.subscriptions.ledger` with the
  path and the cause until the file is repaired. Writing an empty replacement
  would destroy the only record of the month's usage.
- **No home directory to resolve the default path:** the same refusal, naming
  the missing `HOME`.

## Other state

`$BRAMA_STATE_DIR/journal.jsonl` contains retirement and quality records.
`$BRAMA_STATE_DIR/perf.json` (`BRAMA_PERF_PATH` names another) contains replaceable per-process
telemetry; a file that cannot be read, parsed or written is logged as a
`perf_stats_*` warning with its path and cause. The entitlements router owns
encrypted subscription credential storage in managed deployments.
