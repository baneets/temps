import type { Command } from 'commander'
import { overview } from './overview.js'
import { breakdown } from './breakdown.js'

export function registerAnalyticsCommands(program: Command): void {
  const analytics = program
    .command('analytics')
    .alias('stats')
    .description('View project analytics')

  analytics
    .command('overview')
    .alias('o')
    .description('Show analytics dashboard overview')
    .option('-p, --project <project>', 'Project slug or ID')
    .option('--period <period>', 'Time period: today, <n>h, <n>d, <n>m (e.g. 1h, 6h, 48h, 7d, 30d, 3m)', '24h')
    .option('--json', 'Output in JSON format')
    .action(overview)

  analytics
    .command('top <dimension>')
    .description(
      'Show breakdown by dimension: pages, referrers, browsers, os, devices, countries, regions, cities, channels, events, languages, utm_source, utm_medium, utm_campaign'
    )
    .option('-p, --project <project>', 'Project slug or ID')
    .option('--period <period>', 'Time period: today, <n>h, <n>d, <n>m (e.g. 1h, 6h, 48h, 7d, 30d, 3m)', '24h')
    .option('--limit <n>', 'Number of results (default: 20, max: 100)')
    .option('--json', 'Output in JSON format')
    .action(breakdown)

  // Default: no subcommand shows help with available commands
  analytics.addHelpText(
    'after',
    `
Examples:
  $ temps analytics                              Show overview (last 24h)
  $ temps analytics overview -p my-app --period 7d
  $ temps analytics top pages -p my-app --period 30d
  $ temps analytics top referrers --period 1h
  $ temps analytics top browsers --period 48h --json
  $ temps analytics top countries --period 3m --limit 50`
  )
}
