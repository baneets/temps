//! Detects AI agent / LLM crawler user agents and classifies them by provider.
//!
//! Unlike the generic [`crate::crawler_detector::CrawlerDetector`], which only
//! returns whether a request *looks* like a bot, this module returns a stable
//! `(provider, agent)` pair so the UI can surface logos, group by provider, and
//! filter the request log to "show me everything from OpenAI / ChatGPT-User".
//!
//! User-agent strings are trivially spoofable. This detector is meant for
//! observability ("how much of my traffic is AI?"), not for blocking decisions.

use once_cell::sync::Lazy;
use regex::RegexSet;

/// One row in the AI-agent taxonomy.
#[derive(Debug, Clone, Copy)]
pub struct AiAgentMatch {
    /// Vendor / company behind the crawler (e.g. `"OpenAI"`).
    pub provider: &'static str,
    /// Canonical agent name (e.g. `"GPTBot"`, `"ChatGPT-User"`).
    pub agent: &'static str,
    /// What the crawler is doing (training, search, user-initiated fetch).
    pub purpose: AiAgentPurpose,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AiAgentPurpose {
    Training,
    Search,
    UserFetch,
    Mixed,
}

impl AiAgentPurpose {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Training => "training",
            Self::Search => "search",
            Self::UserFetch => "user_fetch",
            Self::Mixed => "mixed",
        }
    }
}

/// Ordered pattern → metadata table. The order matters: more specific tokens
/// (e.g. `OAI-SearchBot`) must come before more generic ones (`openai/`).
const AGENT_PATTERNS: &[(&str, AiAgentMatch)] = &[
    // OpenAI
    (
        r"(?i)\bGPTBot\b",
        AiAgentMatch {
            provider: "OpenAI",
            agent: "GPTBot",
            purpose: AiAgentPurpose::Training,
        },
    ),
    (
        r"(?i)\bOAI-SearchBot\b",
        AiAgentMatch {
            provider: "OpenAI",
            agent: "OAI-SearchBot",
            purpose: AiAgentPurpose::Search,
        },
    ),
    (
        r"(?i)\bChatGPT-User\b",
        AiAgentMatch {
            provider: "OpenAI",
            agent: "ChatGPT-User",
            purpose: AiAgentPurpose::UserFetch,
        },
    ),
    (
        r"(?i)\bopenai/",
        AiAgentMatch {
            provider: "OpenAI",
            agent: "OpenAI",
            purpose: AiAgentPurpose::Mixed,
        },
    ),
    // Anthropic
    (
        r"(?i)\bClaudeBot\b",
        AiAgentMatch {
            provider: "Anthropic",
            agent: "ClaudeBot",
            purpose: AiAgentPurpose::Training,
        },
    ),
    (
        r"(?i)\bClaude-SearchBot\b",
        AiAgentMatch {
            provider: "Anthropic",
            agent: "Claude-SearchBot",
            purpose: AiAgentPurpose::Search,
        },
    ),
    (
        r"(?i)\bClaude-User\b",
        AiAgentMatch {
            provider: "Anthropic",
            agent: "Claude-User",
            purpose: AiAgentPurpose::UserFetch,
        },
    ),
    (
        r"(?i)\banthropic-ai\b",
        AiAgentMatch {
            provider: "Anthropic",
            agent: "anthropic-ai",
            purpose: AiAgentPurpose::Mixed,
        },
    ),
    // Perplexity
    (
        r"(?i)\bPerplexityBot\b",
        AiAgentMatch {
            provider: "Perplexity",
            agent: "PerplexityBot",
            purpose: AiAgentPurpose::Search,
        },
    ),
    (
        r"(?i)\bPerplexity-User\b",
        AiAgentMatch {
            provider: "Perplexity",
            agent: "Perplexity-User",
            purpose: AiAgentPurpose::UserFetch,
        },
    ),
    // Google AI surfaces — Google-Extended is a robots.txt token, not a UA,
    // but GoogleOther is a real UA they use for non-Search fetching.
    (
        r"(?i)\bGoogleOther\b",
        AiAgentMatch {
            provider: "Google",
            agent: "GoogleOther",
            purpose: AiAgentPurpose::Mixed,
        },
    ),
    // Apple
    (
        r"(?i)\bApplebot-Extended\b",
        AiAgentMatch {
            provider: "Apple",
            agent: "Applebot-Extended",
            purpose: AiAgentPurpose::Training,
        },
    ),
    (
        r"(?i)\bApplebot\b",
        AiAgentMatch {
            provider: "Apple",
            agent: "Applebot",
            purpose: AiAgentPurpose::Search,
        },
    ),
    // Meta
    (
        r"(?i)\bMeta-ExternalAgent\b",
        AiAgentMatch {
            provider: "Meta",
            agent: "Meta-ExternalAgent",
            purpose: AiAgentPurpose::Training,
        },
    ),
    (
        r"(?i)\bMeta-ExternalFetcher\b",
        AiAgentMatch {
            provider: "Meta",
            agent: "Meta-ExternalFetcher",
            purpose: AiAgentPurpose::UserFetch,
        },
    ),
    // Amazon
    (
        r"(?i)\bAmazonbot\b",
        AiAgentMatch {
            provider: "Amazon",
            agent: "Amazonbot",
            purpose: AiAgentPurpose::Mixed,
        },
    ),
    // ByteDance
    (
        r"(?i)\bBytespider\b",
        AiAgentMatch {
            provider: "ByteDance",
            agent: "Bytespider",
            purpose: AiAgentPurpose::Training,
        },
    ),
    // Common Crawl
    (
        r"(?i)\bCCBot\b",
        AiAgentMatch {
            provider: "Common Crawl",
            agent: "CCBot",
            purpose: AiAgentPurpose::Training,
        },
    ),
    // Cohere
    (
        r"(?i)\bcohere-ai\b",
        AiAgentMatch {
            provider: "Cohere",
            agent: "cohere-ai",
            purpose: AiAgentPurpose::Mixed,
        },
    ),
    (
        r"(?i)\bcohere-training-data-crawler\b",
        AiAgentMatch {
            provider: "Cohere",
            agent: "cohere-training-data-crawler",
            purpose: AiAgentPurpose::Training,
        },
    ),
    // Diffbot
    (
        r"(?i)\bDiffbot\b",
        AiAgentMatch {
            provider: "Diffbot",
            agent: "Diffbot",
            purpose: AiAgentPurpose::Mixed,
        },
    ),
    // You.com
    (
        r"(?i)\bYouBot\b",
        AiAgentMatch {
            provider: "You.com",
            agent: "YouBot",
            purpose: AiAgentPurpose::Search,
        },
    ),
    // DuckDuckGo (DuckAssist)
    (
        r"(?i)\bDuckAssistBot\b",
        AiAgentMatch {
            provider: "DuckDuckGo",
            agent: "DuckAssistBot",
            purpose: AiAgentPurpose::Search,
        },
    ),
    // Brave
    (
        r"(?i)\bBravebot\b",
        AiAgentMatch {
            provider: "Brave",
            agent: "Bravebot",
            purpose: AiAgentPurpose::Search,
        },
    ),
    // Andi
    (
        r"(?i)\bAndibot\b",
        AiAgentMatch {
            provider: "Andi",
            agent: "Andibot",
            purpose: AiAgentPurpose::Search,
        },
    ),
    // Omgili / Webz.io
    (
        r"(?i)\bOmgilibot\b",
        AiAgentMatch {
            provider: "Omgili",
            agent: "Omgilibot",
            purpose: AiAgentPurpose::Training,
        },
    ),
    (
        r"(?i)\bomgili\b",
        AiAgentMatch {
            provider: "Omgili",
            agent: "Omgili",
            purpose: AiAgentPurpose::Training,
        },
    ),
    // ImageSift
    (
        r"(?i)\bImagesiftBot\b",
        AiAgentMatch {
            provider: "ImageSift",
            agent: "ImagesiftBot",
            purpose: AiAgentPurpose::Training,
        },
    ),
    // Timpi
    (
        r"(?i)\bTimpibot\b",
        AiAgentMatch {
            provider: "Timpi",
            agent: "Timpibot",
            purpose: AiAgentPurpose::Search,
        },
    ),
    // Kangaroo
    (
        r"(?i)\bKangaroo Bot\b",
        AiAgentMatch {
            provider: "Kangaroo",
            agent: "Kangaroo Bot",
            purpose: AiAgentPurpose::Mixed,
        },
    ),
    // Mistral
    (
        r"(?i)\bMistralAI-User\b",
        AiAgentMatch {
            provider: "Mistral",
            agent: "MistralAI-User",
            purpose: AiAgentPurpose::UserFetch,
        },
    ),
    // xAI / Grok
    (
        r"(?i)\bGrokBot\b",
        AiAgentMatch {
            provider: "xAI",
            agent: "GrokBot",
            purpose: AiAgentPurpose::Training,
        },
    ),
];

/// Compiled multi-pattern regex set. `RegexSet::matches` returns the indices of
/// every pattern that matched, so we can find the most specific entry in a
/// single pass.
static AGENT_REGEX_SET: Lazy<RegexSet> = Lazy::new(|| {
    let patterns: Vec<&str> = AGENT_PATTERNS.iter().map(|(p, _)| *p).collect();
    RegexSet::new(&patterns).expect("Failed to compile AI agent regex set")
});

/// All known agents (used by the frontend dropdown).
pub fn known_agents() -> &'static [(&'static str, AiAgentMatch)] {
    AGENT_PATTERNS
}

/// Identify the AI agent behind a user-agent string. Returns `None` for any
/// request that isn't from a known AI agent.
pub fn detect(user_agent: Option<&str>) -> Option<AiAgentMatch> {
    let ua = user_agent?.trim();
    if ua.is_empty() {
        return None;
    }
    let matches: Vec<usize> = AGENT_REGEX_SET.matches(ua).into_iter().collect();
    // Earliest pattern wins — the table is intentionally ordered most specific
    // first so e.g. `OAI-SearchBot` is preferred over `openai/`.
    matches.into_iter().min().map(|idx| AGENT_PATTERNS[idx].1)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_gptbot() {
        let m = detect(Some(
            "Mozilla/5.0 AppleWebKit/537.36 (KHTML, like Gecko); compatible; GPTBot/1.2; +https://openai.com/gptbot",
        ))
        .expect("should detect GPTBot");
        assert_eq!(m.provider, "OpenAI");
        assert_eq!(m.agent, "GPTBot");
        assert_eq!(m.purpose, AiAgentPurpose::Training);
    }

    #[test]
    fn detects_chatgpt_user_over_generic_openai() {
        let m = detect(Some(
            "Mozilla/5.0 (compatible; ChatGPT-User/1.0; +https://openai.com/bot)",
        ))
        .expect("should detect ChatGPT-User");
        assert_eq!(m.agent, "ChatGPT-User");
        assert_eq!(m.purpose, AiAgentPurpose::UserFetch);
    }

    #[test]
    fn detects_claudebot() {
        let m = detect(Some(
            "Mozilla/5.0 (compatible; ClaudeBot/1.0; +https://www.anthropic.com)",
        ))
        .expect("should detect ClaudeBot");
        assert_eq!(m.provider, "Anthropic");
        assert_eq!(m.agent, "ClaudeBot");
    }

    #[test]
    fn detects_perplexity() {
        let m = detect(Some("PerplexityBot/1.0")).expect("should detect PerplexityBot");
        assert_eq!(m.provider, "Perplexity");
    }

    #[test]
    fn detects_meta_external_agent() {
        let m = detect(Some("meta-externalagent/1.1")).expect("should detect Meta-ExternalAgent");
        assert_eq!(m.provider, "Meta");
        assert_eq!(m.agent, "Meta-ExternalAgent");
    }

    #[test]
    fn ignores_regular_browsers() {
        assert!(detect(Some(
            "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/131.0.0.0 Safari/537.36"
        ))
        .is_none());
    }

    #[test]
    fn ignores_empty_and_missing_ua() {
        assert!(detect(None).is_none());
        assert!(detect(Some("")).is_none());
        assert!(detect(Some("   ")).is_none());
    }
}
