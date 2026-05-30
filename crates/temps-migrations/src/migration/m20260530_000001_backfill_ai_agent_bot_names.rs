use sea_orm_migration::prelude::*;

/// One-shot backfill of `proxy_logs.bot_name` with canonical AI-agent names.
///
/// Until the live ingest path was fixed (the `ProxyLogBatchWriter` only ran the
/// loose `CrawlerDetector`, which stores a UA *substring* such as `ClaudeBot/1.0`
/// -> `"Bot/"`), AI crawlers were never tagged with their canonical taxonomy
/// name. The AI Agents analytics page filters `bot_name = ANY(known_agents)`, so
/// those substring values never matched and the page stayed empty. The code fix
/// classifies new rows going forward; this migration reclassifies the rows that
/// already exist so the page has history on day one.
///
/// ## Why only the last 7 days, and why a single-table UPDATE
///
/// `proxy_logs` is a TimescaleDB hypertable (see
/// `m20260225_000001_add_proxy_logs_retention`):
///   - compression policy: chunks older than **7 days** are compressed;
///   - retention policy: chunks older than **30 days** are dropped entirely.
///
/// Updates that touch compressed chunks force TimescaleDB to decompress tuples,
/// which trips `max_tuples_decompressed_per_dml_transaction` (default 100k) and
/// aborts. We avoid that two ways:
///   1. Scope to `timestamp >= now() - interval '7 days'` so chunk exclusion
///      skips every compressed chunk before any row is touched.
///   2. Use a **plain single-table `UPDATE`** with the AI-agent regex in the
///      `WHERE` clause — NOT `UPDATE ... FROM (SELECT ... FROM proxy_logs ...)`.
///      The self-join form materialises the inner scan across chunk boundaries
///      and decompresses (verified to fail locally with 96 compressed chunks);
///      the single-table form lets the planner exclude compressed chunks and
///      only writes rows that actually match an AI agent.
///
/// Anything older than 7 days is compressed (left as-is) or already dropped; the
/// going-forward code fix covers everything ingested after deploy.
///
/// ## Taxonomy
///
/// The `CASE` mirrors `temps_proxy::ai_agent_detector::AGENT_PATTERNS` exactly —
/// all 32 agents across 21 providers — in the same specificity order the Rust
/// detector uses (more specific tokens first, e.g. `OAI-SearchBot` before
/// `openai/`, `Applebot-Extended` before `Applebot`,
/// `cohere-training-data-crawler` before `cohere-ai`, `Omgilibot` before
/// `omgili`). Postgres `~*` is case-insensitive POSIX regex; `\y` is the
/// word-boundary anchor (equivalent to the Rust `\b`).
///
/// Idempotent: re-running only rewrites rows whose `bot_name` differs from the
/// canonical value, and the `\y`-anchored matches are stable.
#[derive(DeriveMigrationName)]
pub struct Migration;

/// Shared classification expression used by the UPDATE. Returns the canonical
/// agent name for a matching `user_agent`, or NULL when no AI agent matches.
const CLASSIFY_CASE: &str = r#"
    CASE
        -- OpenAI (specific tokens before the generic `openai/`)
        WHEN user_agent ~* '\yGPTBot\y'                       THEN 'GPTBot'
        WHEN user_agent ~* '\yOAI-SearchBot\y'                THEN 'OAI-SearchBot'
        WHEN user_agent ~* '\yChatGPT-User\y'                 THEN 'ChatGPT-User'
        WHEN user_agent ~* 'openai/'                          THEN 'OpenAI'
        -- Anthropic
        WHEN user_agent ~* '\yClaudeBot\y'                    THEN 'ClaudeBot'
        WHEN user_agent ~* '\yClaude-SearchBot\y'             THEN 'Claude-SearchBot'
        WHEN user_agent ~* '\yClaude-User\y'                  THEN 'Claude-User'
        WHEN user_agent ~* '\yanthropic-ai\y'                 THEN 'anthropic-ai'
        -- Perplexity
        WHEN user_agent ~* '\yPerplexityBot\y'                THEN 'PerplexityBot'
        WHEN user_agent ~* '\yPerplexity-User\y'              THEN 'Perplexity-User'
        -- Google
        WHEN user_agent ~* '\yGoogleOther\y'                  THEN 'GoogleOther'
        -- Apple (Extended before base)
        WHEN user_agent ~* '\yApplebot-Extended\y'            THEN 'Applebot-Extended'
        WHEN user_agent ~* '\yApplebot\y'                     THEN 'Applebot'
        -- Meta
        WHEN user_agent ~* '\yMeta-ExternalAgent\y'           THEN 'Meta-ExternalAgent'
        WHEN user_agent ~* '\yMeta-ExternalFetcher\y'         THEN 'Meta-ExternalFetcher'
        -- Amazon / ByteDance / Common Crawl
        WHEN user_agent ~* '\yAmazonbot\y'                    THEN 'Amazonbot'
        WHEN user_agent ~* '\yBytespider\y'                   THEN 'Bytespider'
        WHEN user_agent ~* '\yCCBot\y'                        THEN 'CCBot'
        -- Cohere (training-data-crawler before base)
        WHEN user_agent ~* '\ycohere-training-data-crawler\y' THEN 'cohere-training-data-crawler'
        WHEN user_agent ~* '\ycohere-ai\y'                    THEN 'cohere-ai'
        -- Diffbot / You.com / DuckDuckGo / Brave / Andi
        WHEN user_agent ~* '\yDiffbot\y'                      THEN 'Diffbot'
        WHEN user_agent ~* '\yYouBot\y'                       THEN 'YouBot'
        WHEN user_agent ~* '\yDuckAssistBot\y'                THEN 'DuckAssistBot'
        WHEN user_agent ~* '\yBravebot\y'                     THEN 'Bravebot'
        WHEN user_agent ~* '\yAndibot\y'                      THEN 'Andibot'
        -- Omgili (Omgilibot before omgili)
        WHEN user_agent ~* '\yOmgilibot\y'                    THEN 'Omgilibot'
        WHEN user_agent ~* '\yomgili\y'                       THEN 'Omgili'
        -- ImageSift / Timpi / Kangaroo / Mistral / xAI
        WHEN user_agent ~* '\yImagesiftBot\y'                 THEN 'ImagesiftBot'
        WHEN user_agent ~* '\yTimpibot\y'                     THEN 'Timpibot'
        WHEN user_agent ~* '\yKangaroo Bot\y'                 THEN 'Kangaroo Bot'
        WHEN user_agent ~* '\yMistralAI-User\y'               THEN 'MistralAI-User'
        WHEN user_agent ~* '\yGrokBot\y'                      THEN 'GrokBot'
        ELSE NULL
    END
"#;

/// Single combined regex matching ANY known AI-agent token. Used in the UPDATE's
/// `WHERE` so the statement only touches rows that actually match an AI agent —
/// keeping the write set small and letting TimescaleDB exclude compressed chunks
/// instead of decompressing them.
const ANY_AI_AGENT_REGEX: &str = r"\y(GPTBot|OAI-SearchBot|ChatGPT-User|ClaudeBot|Claude-SearchBot|Claude-User|anthropic-ai|PerplexityBot|Perplexity-User|GoogleOther|Applebot-Extended|Applebot|Meta-ExternalAgent|Meta-ExternalFetcher|Amazonbot|Bytespider|CCBot|cohere-training-data-crawler|cohere-ai|Diffbot|YouBot|DuckAssistBot|Bravebot|Andibot|Omgilibot|omgili|ImagesiftBot|Timpibot|Kangaroo Bot|MistralAI-User|GrokBot)\y|openai/";

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let db = manager.get_connection();

        // Plain single-table UPDATE — NOT `UPDATE ... FROM (SELECT ...)`. The
        // self-join form materialises a scan across chunk boundaries and forces
        // TimescaleDB to decompress (verified to abort with "tuple decompression
        // limit exceeded" against a compressed hypertable). This form lets the
        // planner exclude compressed chunks via the `timestamp` predicate, and
        // the `ANY_AI_AGENT_REGEX` WHERE clause restricts writes to rows that
        // actually match an AI agent. The `CASE` then resolves the most specific
        // canonical name. `IS DISTINCT FROM` keeps it idempotent.
        let sql = format!(
            r#"
UPDATE proxy_logs
SET bot_name = ({classify}),
    is_bot   = true
WHERE timestamp >= now() - interval '7 days'
  AND user_agent IS NOT NULL
  AND user_agent ~* '{any_ai}'
  AND bot_name IS DISTINCT FROM ({classify});
"#,
            classify = CLASSIFY_CASE,
            any_ai = ANY_AI_AGENT_REGEX,
        );

        db.execute_unprepared(&sql).await?;

        Ok(())
    }

    async fn down(&self, _manager: &SchemaManager) -> Result<(), DbErr> {
        // Non-reversible: the original (incorrect) substring `bot_name` values
        // are not recoverable, and reverting canonical names back to substrings
        // would only re-break the analytics page. This is a data-quality
        // correction, so `down` is intentionally a no-op.
        Ok(())
    }
}
