-- 001_model_router_clients.sql
--
-- Architectural fix: separate generic HMAC service identities from the
-- trading-autonomy trade-agent registry.
--
-- Background:
--   The model-router gateway (src/gateway/supabase.rs) authenticates
--   x-agent-id callers by looking up an auth_secret in Supabase. Before
--   this migration it read only from `trade_agent_secrets`, a table
--   intended for trading-autonomy's actual trade-decision agents (each
--   row has ticker, mode, status semantic fields). Non-trade-agent
--   services that wanted to call model-router (wisent-app, soon
--   wisent-compute, growth-tactics, etc.) were shoehorned in as fake
--   trade-agent rows with mode='service' and ticker placeholders like
--   'WAPP'. This conflated two distinct concerns and polluted the
--   trade_agents listing with non-trade entries.
--
-- This migration:
--   - Creates `model_router_clients`, the canonical table for service
--     HMAC identities. agent_id is the primary key (matches the
--     x-agent-id header). auth_secret is the 32-byte hex secret.
--     created_at and description are bookkeeping.
--   - The matching Rust change in src/gateway/supabase.rs now queries
--     model_router_clients first, then falls through to
--     trade_agent_secrets for backwards compatibility during migration.
--
-- Migration steps (in order):
--   1. Apply this migration (creates the empty table).
--   2. Build + deploy model-router (the Rust change reads the new
--      table first; the legacy path keeps wisent-app working).
--   3. Backfill: copy wisent-app's row (and any other mode='service'
--      row) from trade_agent_secrets into model_router_clients.
--   4. Verify wisent-app's HMAC requests still succeed (the new path
--      wins because of the lookup order).
--   5. Drop mode='service' rows from trade_agent_secrets +
--      trade_agents once all clients are confirmed migrated.
--   6. Remove the legacy lookup function in supabase.rs in a
--      follow-up Rust change.

CREATE TABLE IF NOT EXISTS public.model_router_clients (
    agent_id      text         PRIMARY KEY,
    auth_secret   text         NOT NULL,
    created_at    timestamptz  NOT NULL DEFAULT now(),
    description   text
);

COMMENT ON TABLE public.model_router_clients IS
    'Canonical HMAC service identities authorized to call model-router. '
    'Separate from trade_agent_secrets which is for trading-autonomy '
    'trade-decision agents only.';

COMMENT ON COLUMN public.model_router_clients.agent_id IS
    'Matches the x-agent-id header on signed requests. Lowercase, '
    'kebab-case service name (e.g. wisent-app, wisent-compute).';

COMMENT ON COLUMN public.model_router_clients.auth_secret IS
    '32-byte hex random. Shared secret used to HMAC-SHA256 sign the '
    'message {agent_id}:{timestamp}:{sha256_hex(body)}.';

-- Enable RLS, then create a service-role-only policy. The gateway uses
-- the SUPABASE_SERVICE_ROLE_KEY which bypasses RLS, so reads work; this
-- locks out anon/authenticated keys from reading or writing.
ALTER TABLE public.model_router_clients ENABLE ROW LEVEL SECURITY;

DROP POLICY IF EXISTS "service role only" ON public.model_router_clients;
CREATE POLICY "service role only"
    ON public.model_router_clients
    FOR ALL
    TO service_role
    USING (true)
    WITH CHECK (true);
