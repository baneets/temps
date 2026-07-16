import { Badge } from '@/components/ui/badge'
import { Button } from '@/components/ui/button'
import {
  Card,
  CardContent,
  CardDescription,
  CardFooter,
  CardHeader,
  CardTitle,
} from '@/components/ui/card'
import { Skeleton } from '@/components/ui/skeleton'
import { Loader2, RotateCcw, Sparkles } from 'lucide-react'
import type { Insight, InsightTone } from './types'
import { AiUnavailableError, useAiInsights } from './useAiInsights'
import type { AiInsightContext } from './useAiInsights'

interface InsightsPanelProps {
  /** Stat insights derived from the page's data (see `derive.ts`). */
  insights: Insight[]
  isLoading?: boolean
  /**
   * When set, the panel offers on-demand AI insight generation over this
   * context. Omit to render a stats-only panel.
   */
  aiContext?: AiInsightContext
  description?: string
  emptyText?: string
}

const TONE_DOT: Record<InsightTone, string> = {
  positive: 'bg-success',
  negative: 'bg-destructive',
  neutral: 'bg-muted-foreground/40',
}

function InsightRow({ insight }: { insight: Insight }) {
  return (
    <li className="flex items-start justify-between gap-4 px-6 py-3.5">
      <div className="flex min-w-0 items-start gap-2.5">
        <span
          className={`mt-1.5 size-1.5 shrink-0 rounded-full ${TONE_DOT[insight.tone]}`}
          aria-hidden="true"
        />
        <div className="min-w-0">
          <p className="text-sm font-medium break-words">{insight.title}</p>
          {insight.detail && (
            <p className="text-sm text-muted-foreground break-words">
              {insight.detail}
            </p>
          )}
        </div>
      </div>
      <div className="flex shrink-0 items-center gap-2">
        {insight.source === 'ai' && (
          <Badge
            variant="secondary"
            className="gap-1 py-0.5 pl-1.5 pr-2 font-normal"
          >
            <Sparkles className="size-3 shrink-0" />
            AI
          </Badge>
        )}
        {insight.value && (
          <span className="text-sm font-semibold tabular-nums">
            {insight.value}
          </span>
        )}
      </div>
    </li>
  )
}

function RowSkeleton() {
  return (
    <li className="flex items-start justify-between gap-4 px-6 py-3.5">
      <div className="flex min-w-0 flex-1 items-start gap-2.5">
        <Skeleton className="mt-1.5 size-1.5 shrink-0 rounded-full" />
        <div className="min-w-0 flex-1 space-y-2">
          <Skeleton className="h-4 w-3/5" />
          <Skeleton className="h-3.5 w-4/5" />
        </div>
      </div>
      <Skeleton className="h-4 w-10 shrink-0" />
    </li>
  )
}

/**
 * Shared insights surface for analytics pages. Renders stat insights the
 * page derived from data it already has, plus — when `aiContext` is
 * provided — AI narrative insights generated on demand through the
 * install's AI gateway. AI rows are always labelled, never auto-run, and
 * failures state exactly what went wrong.
 */
export function InsightsPanel({
  insights,
  isLoading,
  aiContext,
  description = 'What stands out in this period.',
  emptyText = 'Not enough data in this period to surface insights.',
}: InsightsPanelProps) {
  const ai = useAiInsights()

  const aiInsights = ai.data ?? []
  const rows = [...insights, ...aiInsights]
  const aiErrorMessage = ai.error
    ? ai.error instanceof AiUnavailableError || ai.error instanceof Error
      ? ai.error.message
      : 'The AI request failed. Try again.'
    : null

  return (
    <Card>
      <CardHeader>
        <div className="flex items-start justify-between gap-2">
          <div>
            <CardTitle>Insights</CardTitle>
            <CardDescription>{description}</CardDescription>
          </div>
          {aiContext && (
            <Button
              variant="outline"
              size="sm"
              className="shrink-0 gap-1.5"
              disabled={isLoading || ai.isPending}
              onClick={() => ai.mutate(aiContext)}
            >
              {ai.isPending ? (
                <Loader2 className="size-4 shrink-0 animate-spin" />
              ) : ai.data ? (
                <RotateCcw className="size-4 shrink-0" />
              ) : (
                <Sparkles className="size-4 shrink-0" />
              )}
              <span className="hidden sm:inline">
                {ai.data ? 'Regenerate' : 'AI insights'}
              </span>
            </Button>
          )}
        </div>
      </CardHeader>
      <CardContent className="p-0">
        {isLoading ? (
          <ul role="list" className="divide-y">
            <RowSkeleton />
            <RowSkeleton />
            <RowSkeleton />
          </ul>
        ) : rows.length === 0 && !ai.isPending && !aiErrorMessage ? (
          <p className="px-6 pb-6 text-sm text-muted-foreground">{emptyText}</p>
        ) : (
          <ul role="list" className="divide-y">
            {rows.map((insight) => (
              <InsightRow key={insight.id} insight={insight} />
            ))}
            {ai.isPending && (
              <>
                <RowSkeleton />
                <RowSkeleton />
              </>
            )}
            {aiErrorMessage && !ai.isPending && (
              <li className="px-6 py-3.5">
                <p className="text-sm text-destructive">{aiErrorMessage}</p>
              </li>
            )}
          </ul>
        )}
      </CardContent>
      {aiInsights.length > 0 && !ai.isPending && (
        <CardFooter className="text-sm leading-none text-muted-foreground">
          AI insights are generated from the stats on this page and may be
          imprecise.
        </CardFooter>
      )}
    </Card>
  )
}
