export type InsightTone = 'positive' | 'negative' | 'neutral'

export type InsightSource = 'stat' | 'ai'

/**
 * One insight rendered by `InsightsPanel`. Stat insights are derived
 * client-side from data the page already fetched (see `derive.ts`);
 * AI insights come from the install's AI gateway (see `useAiInsights.ts`)
 * and are always labelled as such in the UI.
 */
export interface Insight {
  id: string
  source: InsightSource
  tone: InsightTone
  /** Short, concrete claim, e.g. "Chrome carries most of your traffic." */
  title: string
  /** One supporting sentence with the numbers behind the claim. */
  detail?: string
  /** Headline figure shown on the right, e.g. "68%" or "4 PM". */
  value?: string
}
