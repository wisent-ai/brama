-- 003_subscription_router_checks.sql
--
-- Native subscription/runtime checks collected from the actual CLI apps.
--
-- `subscription_router_entries` is the business catalog: plan, renewal,
-- account, cost, and source notes. `subscription_router_checks` is evidence
-- collected from provider tooling such as `claude auth status`, `codex login
-- status`, or Kimi provider config. Keeping these separate avoids counting one
-- paid subscription multiple times just because several checks observed it.

CREATE EXTENSION IF NOT EXISTS pgcrypto;

CREATE TABLE IF NOT EXISTS public.subscription_router_checks (
    id                  uuid         PRIMARY KEY DEFAULT gen_random_uuid(),
    agent_id            text         NOT NULL,
    source              text         NOT NULL DEFAULT 'model-router-native',
    provider            text         NOT NULL,
    service             text         NOT NULL,
    subscription_id     uuid,
    account_identifier  text,
    status              text         NOT NULL DEFAULT 'unknown',
    auth_method         text,
    plan                text,
    check_kind          text         NOT NULL DEFAULT 'auth_status',
    confidence          text         NOT NULL DEFAULT 'observed',
    error               text,
    metadata            jsonb        NOT NULL DEFAULT '{}'::jsonb,
    checked_at          timestamptz  NOT NULL DEFAULT now(),
    created_at          timestamptz  NOT NULL DEFAULT now(),
    updated_at          timestamptz  NOT NULL DEFAULT now()
);

COMMENT ON TABLE public.subscription_router_checks IS
    'Observed provider CLI/app status for subscription credentials. '
    'Evidence only: does not replace billing/catalog subscription rows.';

COMMENT ON COLUMN public.subscription_router_checks.check_kind IS
    'Type of check, e.g. auth_status, config_status, or runtime_call.';

COMMENT ON COLUMN public.subscription_router_checks.confidence IS
    'How strong the signal is: observed, configured, failed, unavailable.';

CREATE UNIQUE INDEX IF NOT EXISTS subscription_router_checks_unique_observation
    ON public.subscription_router_checks (
        agent_id,
        source,
        provider,
        service,
        coalesce(subscription_id::text, ''),
        coalesce(account_identifier, '')
    );

CREATE INDEX IF NOT EXISTS idx_subscription_router_checks_agent_status
    ON public.subscription_router_checks(agent_id, status);

CREATE INDEX IF NOT EXISTS idx_subscription_router_checks_provider_status
    ON public.subscription_router_checks(provider, status);

ALTER TABLE public.subscription_router_checks ENABLE ROW LEVEL SECURITY;

DROP POLICY IF EXISTS "service role only" ON public.subscription_router_checks;
CREATE POLICY "service role only"
    ON public.subscription_router_checks
    FOR ALL
    TO service_role
    USING (true)
    WITH CHECK (true);
