-- 002_subscription_router_entries.sql
--
-- Central subscription catalog owned by model-router.
--
-- `trade_agent_subscriptions` stores runtime credentials/token blobs used by
-- the router to execute CLI-backed subscription models. That table is not a
-- good place for business metadata such as billing amount, renewal date,
-- account identifier, plan, or source notes.
--
-- `subscription_router_entries` is the shared, non-Oko catalog for that
-- business metadata. Oko, reporting jobs, and other services should read the
-- model-router API over this table instead of importing Swift/Oko code or
-- reading local Oko files.

CREATE EXTENSION IF NOT EXISTS pgcrypto;

CREATE TABLE IF NOT EXISTS public.subscription_router_entries (
    id                  uuid         PRIMARY KEY DEFAULT gen_random_uuid(),
    agent_id            text,
    source              text         NOT NULL DEFAULT 'manual',
    provider            text         NOT NULL,
    service             text         NOT NULL,
    account_identifier  text,
    status              text         NOT NULL DEFAULT 'unknown',
    plan                text,
    monthly_cost_usd    numeric,
    period_cost_usd     numeric,
    expires_at          timestamptz,
    last_verified_at    timestamptz,
    metadata            jsonb        NOT NULL DEFAULT '{}'::jsonb,
    created_at          timestamptz  NOT NULL DEFAULT now(),
    updated_at          timestamptz  NOT NULL DEFAULT now()
);

COMMENT ON TABLE public.subscription_router_entries IS
    'Shared model-router-owned catalog of subscription metadata and costs. '
    'Pairs with trade_agent_subscriptions, which stores runtime credentials.';

COMMENT ON COLUMN public.subscription_router_entries.agent_id IS
    'Optional model-router instance id. NULL means global/shared subscription metadata.';

COMMENT ON COLUMN public.subscription_router_entries.source IS
    'Human/source system label, e.g. invoice, billing_portal, weles, manual.';

COMMENT ON COLUMN public.subscription_router_entries.account_identifier IS
    'Email/account/customer identifier. Returned only to authenticated model-router clients.';

COMMENT ON COLUMN public.subscription_router_entries.monthly_cost_usd IS
    'Normalized recurring monthly cost in USD when known.';

COMMENT ON COLUMN public.subscription_router_entries.period_cost_usd IS
    'Actual billed period cost in USD when different from the monthly normalization.';

CREATE UNIQUE INDEX IF NOT EXISTS subscription_router_entries_unique_source_account
    ON public.subscription_router_entries (
        coalesce(agent_id, ''),
        source,
        provider,
        service,
        coalesce(account_identifier, '')
    );

CREATE INDEX IF NOT EXISTS idx_subscription_router_entries_agent_status
    ON public.subscription_router_entries(agent_id, status);

CREATE INDEX IF NOT EXISTS idx_subscription_router_entries_provider_status
    ON public.subscription_router_entries(provider, status);

ALTER TABLE public.subscription_router_entries ENABLE ROW LEVEL SECURITY;

DROP POLICY IF EXISTS "service role only" ON public.subscription_router_entries;
CREATE POLICY "service role only"
    ON public.subscription_router_entries
    FOR ALL
    TO service_role
    USING (true)
    WITH CHECK (true);
