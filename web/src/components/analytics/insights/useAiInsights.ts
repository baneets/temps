import { chatCompletions, listModels } from '@/api/client'
import { useMutation } from '@tanstack/react-query'
import type { Insight, InsightTone } from './types'

/**
 * AI-generated insights, produced by the install's own AI gateway
 * (`/ai/v1/models` + `/ai/v1/chat/completions`). The caller hands over a
 * compact JSON summary of the stats already shown on the page; the model
 * is only asked to narrate that data, never to fetch or invent numbers.
 * Generation is always an explicit user action — nothing runs on mount.
 */
export interface AiInsightContext {
  /** Which analytics surface the stats describe, e.g. "top pages". */
  surface: string
  rangeStart?: string
  rangeEnd?: string
  stats: Record<string, unknown>
}

/** Thrown when the install has no usable AI provider configured. */
export class AiUnavailableError extends Error {
  constructor(message: string) {
    super(message)
    this.name = 'AiUnavailableError'
  }
}

const SYSTEM_PROMPT = [
  'You generate short insights for a web analytics dashboard.',
  'Reply with ONLY a JSON array, no prose and no code fences.',
  'Each element: {"tone": "positive" | "negative" | "neutral", "title": string, "detail": string, "value": string (optional)}.',
  'title: one concrete claim as a full sentence with a period, at most 90 characters.',
  'detail: one supporting sentence citing the exact numbers from the provided stats.',
  'value: an optional short headline figure such as "68%" or "4 PM".',
  'Return 2 to 4 elements. Only make claims the provided stats support.',
  'Prefer non-obvious observations (comparisons, imbalances, opportunities) over restating totals.',
].join(' ')

const TONES: readonly InsightTone[] = ['positive', 'negative', 'neutral']

/** Prefer a small/fast model when the gateway exposes several. */
function pickModel(ids: string[]): string {
  return ids.find((id) => /mini|haiku|flash|lite|nano/i.test(id)) ?? ids[0]
}

function contentToText(content: unknown): string {
  if (typeof content === 'string') return content
  if (Array.isArray(content)) {
    return content
      .map((part) => {
        if (typeof part === 'string') return part
        if (part && typeof part === 'object' && 'text' in part) {
          const text = (part as { text?: unknown }).text
          return typeof text === 'string' ? text : ''
        }
        return ''
      })
      .join('')
  }
  return ''
}

function parseInsights(text: string): Insight[] {
  const start = text.indexOf('[')
  const end = text.lastIndexOf(']')
  if (start === -1 || end <= start) {
    throw new Error('The AI reply did not contain a JSON array of insights.')
  }
  let raw: unknown
  try {
    raw = JSON.parse(text.slice(start, end + 1))
  } catch {
    throw new Error('The AI reply was not valid JSON.')
  }
  if (!Array.isArray(raw)) {
    throw new Error('The AI reply was not a JSON array.')
  }

  const insights: Insight[] = []
  raw.forEach((item, index) => {
    if (!item || typeof item !== 'object') return
    const record = item as Record<string, unknown>
    const title = typeof record.title === 'string' ? record.title.trim() : ''
    if (!title) return
    const tone = TONES.includes(record.tone as InsightTone)
      ? (record.tone as InsightTone)
      : 'neutral'
    const detail =
      typeof record.detail === 'string' && record.detail.trim()
        ? record.detail.trim()
        : undefined
    const value =
      typeof record.value === 'string' && record.value.trim()
        ? record.value.trim()
        : undefined
    insights.push({
      id: `ai-${index}`,
      source: 'ai',
      tone,
      title,
      detail,
      value,
    })
  })

  if (insights.length === 0) {
    throw new Error('The AI reply could not be parsed into insights.')
  }
  return insights.slice(0, 4)
}

function problemMessage(error: unknown, fallback: string): string {
  if (error && typeof error === 'object') {
    const problem = error as { detail?: unknown; title?: unknown }
    if (typeof problem.detail === 'string' && problem.detail)
      return problem.detail
    if (typeof problem.title === 'string' && problem.title) return problem.title
  }
  return fallback
}

async function generateAiInsights(
  context: AiInsightContext
): Promise<Insight[]> {
  const models = await listModels()
  if (models.error) {
    throw new AiUnavailableError(
      problemMessage(
        models.error,
        'The AI gateway is not reachable. Connect an AI provider to enable AI insights.'
      )
    )
  }
  const modelIds = models.data?.data?.map((m) => m.id) ?? []
  if (modelIds.length === 0) {
    throw new AiUnavailableError(
      'No AI models are available. Connect an AI provider to enable AI insights.'
    )
  }

  const completion = await chatCompletions({
    body: {
      model: pickModel(modelIds),
      temperature: 0.2,
      max_tokens: 700,
      messages: [
        { role: 'system', content: SYSTEM_PROMPT },
        {
          role: 'user',
          content: JSON.stringify({
            surface: context.surface,
            date_range: {
              start: context.rangeStart,
              end: context.rangeEnd,
            },
            stats: context.stats,
          }),
        },
      ],
    },
  })
  if (completion.error) {
    throw new Error(
      problemMessage(completion.error, 'The AI request failed. Try again.')
    )
  }

  return parseInsights(
    contentToText(completion.data?.choices?.[0]?.message?.content)
  )
}

export function useAiInsights() {
  return useMutation({ mutationFn: generateAiInsights })
}
