import { ReactNode } from 'react'
import {
  CartesianGrid,
  Line,
  LineChart,
  ReferenceLine,
  XAxis,
  YAxis,
} from 'recharts'
import {
  ChartConfig,
  ChartContainer,
  ChartTooltip,
  ChartTooltipContent,
} from '@/components/ui/chart'
import { cn } from '@/lib/utils'
import type { MetricTone } from './metric-sparkline'

export type ThresholdLineSeries = {
  /** Data key on each point — what to plot. */
  dataKey: string
  /** Stroke tone for the line. Defaults to `neutral` (primary). */
  tone?: MetricTone | 'primary'
  /** Human-readable series label for the tooltip. */
  label: string
}

export type ThresholdBand = {
  /** Y value to draw the reference line at. */
  value: number
  /** Tone picks the color; "good" is emerald, "poor" is red, "warn" is amber. */
  tone: MetricTone
  /** Optional label rendered at the end of the reference line. */
  label?: string
}

interface ThresholdLineChartProps {
  data: any[]
  xKey: string
  series: ThresholdLineSeries
  /** Horizontal reference lines drawn across the chart for pass/fail bands. */
  thresholds?: ThresholdBand[]
  /** Height of the chart in px. Defaults to 300. */
  height?: number
  /** Format the Y-axis ticks (e.g. "2.5s"). */
  yTickFormatter?: (value: number) => string
  /** Format the tooltip value. */
  tooltipValueFormatter?: (value: number) => string
  /** Extra content rendered inside the tooltip under the value. */
  tooltipFooter?: (value: number) => ReactNode
  /**
   * Message rendered instead of the chart when there aren't enough points
   * to draw a line. Defaults to a generic fallback.
   */
  emptyMessage?: ReactNode
  className?: string
}

const SERIES_STROKE: Record<NonNullable<ThresholdLineSeries['tone']>, string> = {
  good: 'var(--chart-2)',
  warn: 'var(--chart-3)',
  poor: 'var(--chart-4)',
  neutral: 'var(--chart-1)',
  primary: 'var(--chart-1)',
}

const THRESHOLD_STROKE: Record<MetricTone, string> = {
  good: 'var(--chart-2)',
  warn: 'var(--chart-3)',
  poor: 'var(--chart-4)',
  neutral: 'var(--muted-foreground)',
}

/**
 * Themed single-series line chart with optional horizontal threshold
 * reference lines (e.g. Core Web Vitals "Good" / "Poor" bands).
 *
 * Built on `ChartContainer` so grid, axis, and tooltip automatically follow
 * the app theme in both light and dark mode.
 */
export function ThresholdLineChart({
  data,
  xKey,
  series,
  thresholds = [],
  height = 300,
  yTickFormatter,
  tooltipValueFormatter,
  tooltipFooter,
  emptyMessage,
  className,
}: ThresholdLineChartProps) {
  const tone = series.tone ?? 'primary'
  const stroke = SERIES_STROKE[tone]

  const config: ChartConfig = {
    [series.dataKey]: {
      label: series.label,
      color: stroke,
    },
  }

  const validCount = data.reduce((n, p) => {
    const v = p?.[series.dataKey]
    return v === null || v === undefined ? n : n + 1
  }, 0)

  if (validCount < 2) {
    return (
      <div
        className={cn(
          'flex w-full items-center justify-center rounded-md border border-dashed text-sm text-muted-foreground',
          className,
        )}
        style={{ height }}
      >
        {emptyMessage ?? (
          <div className="flex flex-col items-center gap-1 px-4 text-center">
            <span className="font-medium text-foreground">
              Not enough data to chart
            </span>
            <span className="text-xs">
              {validCount === 0
                ? 'No samples in this range.'
                : 'Only one sample recorded — a trend needs at least two.'}
            </span>
          </div>
        )}
      </div>
    )
  }

  // Include threshold lines in the Y-axis domain so they're always visible.
  // Recharts auto-fits to data, which pushes out-of-range threshold lines
  // off the chart — e.g. a 488ms LCP never reveals the 2500ms/4000ms bands.
  const numericValues = data
    .map((p) => p?.[series.dataKey])
    .filter((v): v is number => typeof v === 'number')
  const dataMax = numericValues.length ? Math.max(...numericValues) : 0
  const dataMin = numericValues.length ? Math.min(...numericValues) : 0
  const thresholdMax = thresholds.reduce(
    (m, t) => Math.max(m, t.value),
    dataMax,
  )
  const yMax = thresholdMax * 1.1
  const yMin = Math.min(0, dataMin)

  return (
    <ChartContainer
      config={config}
      className={cn('aspect-auto w-full', className)}
      style={{ height }}
    >
      <LineChart
        data={data}
        margin={{ top: 12, right: 24, left: 8, bottom: 0 }}
      >
        <CartesianGrid
          strokeDasharray="3 3"
          vertical={false}
          className="stroke-border"
          strokeOpacity={0.6}
        />
        <XAxis
          dataKey={xKey}
          tickLine={false}
          axisLine={false}
          tickMargin={8}
          minTickGap={32}
          className="text-xs"
        />
        <YAxis
          tickLine={false}
          axisLine={false}
          tickMargin={8}
          width={52}
          domain={[yMin, yMax]}
          tickFormatter={yTickFormatter}
          className="text-xs"
        />
        <ChartTooltip
          cursor={{ strokeDasharray: '3 3' }}
          content={
            <ChartTooltipContent
              indicator="line"
              formatter={(value) => {
                const num = value as number
                return (
                  <div className="flex flex-col gap-0.5">
                    <span className="font-mono font-medium text-foreground">
                      {tooltipValueFormatter
                        ? tooltipValueFormatter(num)
                        : num.toLocaleString()}
                    </span>
                    {tooltipFooter ? (
                      <span className="text-[10px] text-muted-foreground">
                        {tooltipFooter(num)}
                      </span>
                    ) : null}
                  </div>
                )
              }}
            />
          }
        />
        {thresholds.map((t, idx) => (
          <ReferenceLine
            key={`${t.tone}-${idx}`}
            y={t.value}
            stroke={THRESHOLD_STROKE[t.tone]}
            strokeDasharray="4 4"
            strokeOpacity={0.7}
            label={
              t.label
                ? {
                    value: t.label,
                    position: 'right',
                    fill: THRESHOLD_STROKE[t.tone],
                    fontSize: 10,
                  }
                : undefined
            }
          />
        ))}
        <Line
          type="monotone"
          dataKey={series.dataKey}
          stroke={`var(--color-${series.dataKey})`}
          strokeWidth={2}
          dot={validCount <= 8 ? { r: 3, strokeWidth: 0 } : false}
          activeDot={{ r: 4, strokeWidth: 0 }}
          connectNulls
          isAnimationActive={false}
        />
      </LineChart>
    </ChartContainer>
  )
}
