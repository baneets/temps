import { getClient } from '../api/index.js';
import {
  ok,
  json,
  table,
  handleToolCall,
  requireParam,
  optionalParam,
} from './_helpers.js';
import type { ToolDefinition } from '../types/index.js';

// ── Response interfaces ──────────────────────────────────────────

interface UniqueCountsResponse {
  count: number;
}

interface HasEventsResponse {
  has_events: boolean;
}

interface EventTimeline {
  date: string;
  count: number;
}

interface EventCount {
  event_name: string;
  count: number;
  percentage: number;
}

interface EventTypeBreakdown {
  event_type: string;
  count: number;
  percentage: number;
}

interface PropertyBreakdownItem {
  value: string;
  count: number;
  percentage: number;
}

interface PropertyBreakdownResponse {
  property: string;
  items: PropertyBreakdownItem[];
  total: number;
}

interface PropertyTimelineItem {
  timestamp: string;
  value: string;
  count: number;
}

interface PropertyTimelineResponse {
  property: string;
  bucket_size: string;
  items: PropertyTimelineItem[];
}

interface ActiveVisitorsResponse {
  active_visitors: number;
  window_minutes: number;
}

interface AggregatedBucketItem {
  timestamp: string;
  count: number;
}

interface AggregatedBucketsResponse {
  bucket_size: string;
  aggregation_level: string;
  items: AggregatedBucketItem[];
  total: number;
}

interface GeneralStatsResponse {
  total_unique_visitors: number;
  total_visits: number;
  total_page_views: number;
  total_events: number;
  total_projects: number;
  avg_bounce_rate: number;
  avg_engagement_rate: number;
  previous_unique_visitors: number | null;
  previous_page_views: number | null;
  visitors_trend_percentage: number | null;
  page_views_trend_percentage: number | null;
  project_breakdown: Array<{
    project_id: number;
    project_name: string | null;
    unique_visitors: number;
    total_visits: number;
    total_page_views: number;
    bounce_rate: number;
    engagement_rate: number;
  }>;
}

interface PagePathInfo {
  page_path: string;
  session_count: number;
  page_view_count: number;
  avg_time_seconds: number | null;
  first_seen: string;
  last_seen: string;
}

interface PagePathsResponse {
  page_paths: PagePathInfo[];
  total_count: number;
}

interface PageActivityBucket {
  timestamp: string;
  visitors: number;
  page_views: number;
  avg_time_seconds: number;
}

interface PageCountryStats {
  country: string;
  country_code: string | null;
  visitors: number;
  page_views: number;
  percentage: number;
}

interface PageReferrerStats {
  referrer: string;
  visits: number;
  percentage: number;
}

interface PagePathDetailResponse {
  page_path: string;
  unique_visitors: number;
  total_page_views: number;
  avg_time_on_page: number;
  bounce_rate: number;
  entry_rate: number;
  exit_rate: number;
  activity_over_time: PageActivityBucket[];
  countries: PageCountryStats[];
  referrers: PageReferrerStats[];
  bucket_interval: string;
}

interface PageFlowEntry {
  page_path: string;
  count: number;
  percentage: number;
  bounce_rate?: number;
}

interface PageTransition {
  from_path: string;
  to_path: string;
  count: number;
}

interface DropOffPoint {
  page_path: string;
  views: number;
  drop_off_count: number;
  drop_off_rate: number;
}

interface PageFlowResponse {
  top_entry_pages: PageFlowEntry[];
  top_exit_pages: PageFlowEntry[];
  drop_off_points: DropOffPoint[];
  transitions: PageTransition[];
  total_pages: number;
  total_sessions: number;
}

// ── Tools ────────────────────────────────────────────────────────

export const tools: ToolDefinition[] = [
  // ── has_analytics_events ───────────────────────────────────────
  {
    name: 'has_analytics_events',
    description:
      'Check whether a project has any analytics events recorded. Useful to verify tracking is working before querying metrics.',
    inputSchema: {
      type: 'object',
      properties: {
        project_id: {
          type: 'number',
          description: 'The project ID',
        },
      },
      required: ['project_id'],
    },
    handler: (args) =>
      handleToolCall(async () => {
        const client = getClient();
        const projectId = requireParam<number>(args, 'project_id');
        const result = await client.get<HasEventsResponse>(
          `/projects/${projectId}/has-events`,
        );
        return ok(
          result.has_events
            ? `Project ${projectId} has analytics events.`
            : `Project ${projectId} has no analytics events yet.`,
        );
      }),
  },

  // ── get_unique_counts ──────────────────────────────────────────
  {
    name: 'get_unique_counts',
    description:
      'Get a single unique count for a project: unique visitors, sessions, or page views within a date range.',
    inputSchema: {
      type: 'object',
      properties: {
        project_id: {
          type: 'number',
          description: 'The project ID',
        },
        start_date: {
          type: 'string',
          description: 'Start date in ISO 8601 format (e.g. "2026-02-17T00:00:00Z")',
        },
        end_date: {
          type: 'string',
          description: 'End date in ISO 8601 format',
        },
        metric: {
          type: 'string',
          description:
            'Metric to count: "visitors", "sessions", or "page_views". Defaults to "sessions".',
          enum: ['visitors', 'sessions', 'page_views'],
        },
        environment_id: {
          type: 'number',
          description: 'Optional environment ID to filter by',
        },
      },
      required: ['project_id', 'start_date', 'end_date'],
    },
    handler: (args) =>
      handleToolCall(async () => {
        const client = getClient();
        const projectId = requireParam<number>(args, 'project_id');
        const startDate = requireParam<string>(args, 'start_date');
        const endDate = requireParam<string>(args, 'end_date');
        const metric = optionalParam<string>(args, 'metric', 'sessions');
        const envId = optionalParam<number>(args, 'environment_id');

        const query: Record<string, unknown> = {
          start_date: startDate,
          end_date: endDate,
          metric,
        };
        if (envId !== undefined) query.environment_id = envId;

        const result = await client.get<UniqueCountsResponse>(
          `/projects/${projectId}/unique-counts`,
          query,
        );

        return ok(
          `**${metric}** for project ${projectId}: **${result.count.toLocaleString()}**\n\n` +
            `Period: ${startDate} — ${endDate}`,
        );
      }),
  },

  // ── get_active_visitors ────────────────────────────────────────
  {
    name: 'get_active_visitors',
    description:
      'Get the number of currently active visitors on a project (real-time, last 5 minutes).',
    inputSchema: {
      type: 'object',
      properties: {
        project_id: {
          type: 'number',
          description: 'The project ID',
        },
        environment_id: {
          type: 'number',
          description: 'Optional environment ID to filter by',
        },
      },
      required: ['project_id'],
    },
    handler: (args) =>
      handleToolCall(async () => {
        const client = getClient();
        const projectId = requireParam<number>(args, 'project_id');
        const envId = optionalParam<number>(args, 'environment_id');

        const query: Record<string, unknown> = {};
        if (envId !== undefined) query.environment_id = envId;

        const result = await client.get<ActiveVisitorsResponse>(
          `/projects/${projectId}/active-visitors`,
          query,
        );

        return ok(
          `**Active visitors** (last ${result.window_minutes} min): **${result.active_visitors}**`,
        );
      }),
  },

  // ── get_hourly_visits ──────────────────────────────────────────
  {
    name: 'get_hourly_visits',
    description:
      'Get a time series of visits (page views, sessions, or visitors) bucketed by hour for a project.',
    inputSchema: {
      type: 'object',
      properties: {
        project_id: {
          type: 'number',
          description: 'The project ID',
        },
        start_date: {
          type: 'string',
          description: 'Start date in ISO 8601 format',
        },
        end_date: {
          type: 'string',
          description: 'End date in ISO 8601 format',
        },
        aggregation_level: {
          type: 'string',
          description:
            'What to count: "events" (page views, default), "sessions", or "visitors".',
          enum: ['events', 'sessions', 'visitors'],
        },
        environment_id: {
          type: 'number',
          description: 'Optional environment ID to filter by',
        },
      },
      required: ['project_id', 'start_date', 'end_date'],
    },
    handler: (args) =>
      handleToolCall(async () => {
        const client = getClient();
        const projectId = requireParam<number>(args, 'project_id');
        const startDate = requireParam<string>(args, 'start_date');
        const endDate = requireParam<string>(args, 'end_date');
        const aggLevel = optionalParam<string>(args, 'aggregation_level', 'events');
        const envId = optionalParam<number>(args, 'environment_id');

        const query: Record<string, unknown> = {
          start_date: startDate,
          end_date: endDate,
          aggregation_level: aggLevel,
        };
        if (envId !== undefined) query.environment_id = envId;

        const buckets = await client.get<EventTimeline[]>(
          `/projects/${projectId}/hourly-visits`,
          query,
        );

        if (!buckets.length) {
          return ok('No visit data found for this period.');
        }

        const total = buckets.reduce((sum, b) => sum + b.count, 0);
        const peak = buckets.reduce((max, b) => (b.count > max.count ? b : max), buckets[0]);

        const rows = buckets.map((b) => [
          new Date(b.date).toISOString().replace('T', ' ').slice(0, 16),
          String(b.count),
        ]);

        return ok(
          `## Hourly Visits — Project ${projectId}\n\n` +
            `**Total:** ${total.toLocaleString()} | **Peak:** ${peak.count} at ${new Date(peak.date).toISOString().slice(0, 16)}\n\n` +
            table(['Time', 'Count'], rows),
        );
      }),
  },

  // ── get_events_count ───────────────────────────────────────────
  {
    name: 'get_analytics_events',
    description:
      'Get the top events for a project ranked by count, with percentages. Useful to see what users are doing most.',
    inputSchema: {
      type: 'object',
      properties: {
        project_id: {
          type: 'number',
          description: 'The project ID',
        },
        start_date: {
          type: 'string',
          description: 'Start date in ISO 8601 format',
        },
        end_date: {
          type: 'string',
          description: 'End date in ISO 8601 format',
        },
        environment_id: {
          type: 'number',
          description: 'Optional environment ID to filter by',
        },
        limit: {
          type: 'number',
          description: 'Max number of events to return (default 20, max 100)',
        },
        custom_events_only: {
          type: 'boolean',
          description: 'Only return custom events (default true)',
        },
        aggregation_level: {
          type: 'string',
          description: 'Count by: "events" (default), "sessions", or "visitors".',
          enum: ['events', 'sessions', 'visitors'],
        },
      },
      required: ['project_id', 'start_date', 'end_date'],
    },
    handler: (args) =>
      handleToolCall(async () => {
        const client = getClient();
        const projectId = requireParam<number>(args, 'project_id');
        const startDate = requireParam<string>(args, 'start_date');
        const endDate = requireParam<string>(args, 'end_date');
        const envId = optionalParam<number>(args, 'environment_id');
        const limit = optionalParam<number>(args, 'limit');
        const customOnly = optionalParam<boolean>(args, 'custom_events_only');
        const aggLevel = optionalParam<string>(args, 'aggregation_level');

        const query: Record<string, unknown> = {
          start_date: startDate,
          end_date: endDate,
        };
        if (envId !== undefined) query.environment_id = envId;
        if (limit !== undefined) query.limit = limit;
        if (customOnly !== undefined) query.custom_events_only = customOnly;
        if (aggLevel !== undefined) query.aggregation_level = aggLevel;

        const events = await client.get<EventCount[]>(
          `/projects/${projectId}/events`,
          query,
        );

        if (!events.length) {
          return ok('No events found for this period.');
        }

        const rows = events.map((e) => [
          e.event_name,
          e.count.toLocaleString(),
          `${e.percentage.toFixed(1)}%`,
        ]);

        return ok(
          `## Top Events — Project ${projectId}\n\n` +
            table(['Event', 'Count', '%'], rows),
        );
      }),
  },

  // ── get_event_type_breakdown ───────────────────────────────────
  {
    name: 'get_event_type_breakdown',
    description:
      'Get a breakdown of events by type (page_view, custom, etc.) for a project.',
    inputSchema: {
      type: 'object',
      properties: {
        project_id: {
          type: 'number',
          description: 'The project ID',
        },
        start_date: {
          type: 'string',
          description: 'Start date in ISO 8601 format',
        },
        end_date: {
          type: 'string',
          description: 'End date in ISO 8601 format',
        },
        environment_id: {
          type: 'number',
          description: 'Optional environment ID to filter by',
        },
        aggregation_level: {
          type: 'string',
          description: 'Count by: "events" (default), "sessions", or "visitors".',
          enum: ['events', 'sessions', 'visitors'],
        },
      },
      required: ['project_id', 'start_date', 'end_date'],
    },
    handler: (args) =>
      handleToolCall(async () => {
        const client = getClient();
        const projectId = requireParam<number>(args, 'project_id');
        const startDate = requireParam<string>(args, 'start_date');
        const endDate = requireParam<string>(args, 'end_date');
        const envId = optionalParam<number>(args, 'environment_id');
        const aggLevel = optionalParam<string>(args, 'aggregation_level');

        const query: Record<string, unknown> = {
          start_date: startDate,
          end_date: endDate,
        };
        if (envId !== undefined) query.environment_id = envId;
        if (aggLevel !== undefined) query.aggregation_level = aggLevel;

        const types = await client.get<EventTypeBreakdown[]>(
          `/projects/${projectId}/events/breakdown`,
          query,
        );

        if (!types.length) {
          return ok('No event type data found for this period.');
        }

        const rows = types.map((t) => [
          t.event_type,
          t.count.toLocaleString(),
          `${t.percentage.toFixed(1)}%`,
        ]);

        return ok(
          `## Event Type Breakdown — Project ${projectId}\n\n` +
            table(['Type', 'Count', '%'], rows),
        );
      }),
  },

  // ── get_events_timeline ────────────────────────────────────────
  {
    name: 'get_events_timeline',
    description:
      'Get a time series of event counts for a project, optionally filtered by event name. Auto-detects bucket size (hour/day/week).',
    inputSchema: {
      type: 'object',
      properties: {
        project_id: {
          type: 'number',
          description: 'The project ID',
        },
        start_date: {
          type: 'string',
          description: 'Start date in ISO 8601 format',
        },
        end_date: {
          type: 'string',
          description: 'End date in ISO 8601 format',
        },
        event_name: {
          type: 'string',
          description: 'Optional event name to filter by (e.g. "page_view", "click")',
        },
        bucket_size: {
          type: 'string',
          description: 'Time bucket: "hour", "day", or "week" (auto-detected if omitted)',
          enum: ['hour', 'day', 'week'],
        },
        environment_id: {
          type: 'number',
          description: 'Optional environment ID to filter by',
        },
        aggregation_level: {
          type: 'string',
          description: 'Count by: "events" (default), "sessions", or "visitors".',
          enum: ['events', 'sessions', 'visitors'],
        },
      },
      required: ['project_id', 'start_date', 'end_date'],
    },
    handler: (args) =>
      handleToolCall(async () => {
        const client = getClient();
        const projectId = requireParam<number>(args, 'project_id');
        const startDate = requireParam<string>(args, 'start_date');
        const endDate = requireParam<string>(args, 'end_date');
        const eventName = optionalParam<string>(args, 'event_name');
        const bucketSize = optionalParam<string>(args, 'bucket_size');
        const envId = optionalParam<number>(args, 'environment_id');
        const aggLevel = optionalParam<string>(args, 'aggregation_level');

        const query: Record<string, unknown> = {
          start_date: startDate,
          end_date: endDate,
        };
        if (eventName !== undefined) query.event_name = eventName;
        if (bucketSize !== undefined) query.bucket_size = bucketSize;
        if (envId !== undefined) query.environment_id = envId;
        if (aggLevel !== undefined) query.aggregation_level = aggLevel;

        const timeline = await client.get<EventTimeline[]>(
          `/projects/${projectId}/events/timeline`,
          query,
        );

        if (!timeline.length) {
          return ok('No timeline data found for this period.');
        }

        const total = timeline.reduce((sum, b) => sum + b.count, 0);
        const rows = timeline.map((b) => [
          new Date(b.date).toISOString().replace('T', ' ').slice(0, 16),
          String(b.count),
        ]);

        const label = eventName ? `"${eventName}"` : 'All Events';
        return ok(
          `## Events Timeline — ${label} — Project ${projectId}\n\n` +
            `**Total:** ${total.toLocaleString()}\n\n` +
            table(['Time', 'Count'], rows),
        );
      }),
  },

  // ── get_property_breakdown ─────────────────────────────────────
  {
    name: 'get_property_breakdown',
    description:
      'Break down analytics by a property: country, browser, device_type, operating_system, referrer_hostname, page_path, utm_source, utm_medium, utm_campaign, channel, language, and more.',
    inputSchema: {
      type: 'object',
      properties: {
        project_id: {
          type: 'number',
          description: 'The project ID',
        },
        start_date: {
          type: 'string',
          description: 'Start date in ISO 8601 format',
        },
        end_date: {
          type: 'string',
          description: 'End date in ISO 8601 format',
        },
        group_by: {
          type: 'string',
          description:
            'Property to group by. One of: channel, device_type, browser, browser_version, operating_system, operating_system_version, utm_source, utm_medium, utm_campaign, utm_term, utm_content, referrer_hostname, language, event_type, event_name, page_path, pathname, country, region, city.',
          enum: [
            'channel',
            'device_type',
            'browser',
            'browser_version',
            'operating_system',
            'operating_system_version',
            'utm_source',
            'utm_medium',
            'utm_campaign',
            'utm_term',
            'utm_content',
            'referrer_hostname',
            'language',
            'event_type',
            'event_name',
            'page_path',
            'pathname',
            'country',
            'region',
            'city',
          ],
        },
        event_name: {
          type: 'string',
          description: 'Optional event name to filter by',
        },
        environment_id: {
          type: 'number',
          description: 'Optional environment ID to filter by',
        },
        aggregation_level: {
          type: 'string',
          description: 'Count by: "events" (default), "sessions", or "visitors".',
          enum: ['events', 'sessions', 'visitors'],
        },
        limit: {
          type: 'number',
          description: 'Max number of items (default 20, max 100)',
        },
      },
      required: ['project_id', 'start_date', 'end_date', 'group_by'],
    },
    handler: (args) =>
      handleToolCall(async () => {
        const client = getClient();
        const projectId = requireParam<number>(args, 'project_id');
        const startDate = requireParam<string>(args, 'start_date');
        const endDate = requireParam<string>(args, 'end_date');
        const groupBy = requireParam<string>(args, 'group_by');
        const eventName = optionalParam<string>(args, 'event_name');
        const envId = optionalParam<number>(args, 'environment_id');
        const aggLevel = optionalParam<string>(args, 'aggregation_level');
        const limit = optionalParam<number>(args, 'limit');

        const query: Record<string, unknown> = {
          start_date: startDate,
          end_date: endDate,
          group_by: groupBy,
        };
        if (eventName !== undefined) query.event_name = eventName;
        if (envId !== undefined) query.environment_id = envId;
        if (aggLevel !== undefined) query.aggregation_level = aggLevel;
        if (limit !== undefined) query.limit = limit;

        const result = await client.get<PropertyBreakdownResponse>(
          `/projects/${projectId}/events/properties/breakdown`,
          query,
        );

        if (!result.items.length) {
          return ok(`No data for "${groupBy}" breakdown in this period.`);
        }

        const rows = result.items.map((item) => [
          item.value || '(empty)',
          item.count.toLocaleString(),
          `${item.percentage.toFixed(1)}%`,
        ]);

        return ok(
          `## ${groupBy} Breakdown — Project ${projectId}\n\n` +
            `**Total:** ${result.total.toLocaleString()}\n\n` +
            table([groupBy, 'Count', '%'], rows),
        );
      }),
  },

  // ── get_property_timeline ──────────────────────────────────────
  {
    name: 'get_property_timeline',
    description:
      'Get a time series broken down by a property (e.g. browser, country, device_type). Shows how each property value trends over time.',
    inputSchema: {
      type: 'object',
      properties: {
        project_id: {
          type: 'number',
          description: 'The project ID',
        },
        start_date: {
          type: 'string',
          description: 'Start date in ISO 8601 format',
        },
        end_date: {
          type: 'string',
          description: 'End date in ISO 8601 format',
        },
        group_by: {
          type: 'string',
          description: 'Property to group by (same options as get_property_breakdown)',
          enum: [
            'channel',
            'device_type',
            'browser',
            'operating_system',
            'utm_source',
            'utm_medium',
            'utm_campaign',
            'referrer_hostname',
            'language',
            'event_name',
            'page_path',
            'country',
            'region',
            'city',
          ],
        },
        event_name: {
          type: 'string',
          description: 'Optional event name to filter by',
        },
        bucket_size: {
          type: 'string',
          description: 'Time bucket: "hour", "day", or "week"',
          enum: ['hour', 'day', 'week'],
        },
        environment_id: {
          type: 'number',
          description: 'Optional environment ID to filter by',
        },
        aggregation_level: {
          type: 'string',
          description: 'Count by: "events" (default), "sessions", or "visitors".',
          enum: ['events', 'sessions', 'visitors'],
        },
      },
      required: ['project_id', 'start_date', 'end_date', 'group_by'],
    },
    handler: (args) =>
      handleToolCall(async () => {
        const client = getClient();
        const projectId = requireParam<number>(args, 'project_id');
        const startDate = requireParam<string>(args, 'start_date');
        const endDate = requireParam<string>(args, 'end_date');
        const groupBy = requireParam<string>(args, 'group_by');
        const eventName = optionalParam<string>(args, 'event_name');
        const bucketSize = optionalParam<string>(args, 'bucket_size');
        const envId = optionalParam<number>(args, 'environment_id');
        const aggLevel = optionalParam<string>(args, 'aggregation_level');

        const query: Record<string, unknown> = {
          start_date: startDate,
          end_date: endDate,
          group_by: groupBy,
        };
        if (eventName !== undefined) query.event_name = eventName;
        if (bucketSize !== undefined) query.bucket_size = bucketSize;
        if (envId !== undefined) query.environment_id = envId;
        if (aggLevel !== undefined) query.aggregation_level = aggLevel;

        const result = await client.get<PropertyTimelineResponse>(
          `/projects/${projectId}/events/properties/timeline`,
          query,
        );

        if (!result.items.length) {
          return ok(`No timeline data for "${groupBy}" in this period.`);
        }

        // Group items by timestamp for a pivot-style view
        const valueSet = new Set(result.items.map((i) => i.value));
        const values = [...valueSet].slice(0, 10); // cap columns
        const timeMap = new Map<string, Map<string, number>>();

        for (const item of result.items) {
          if (!values.includes(item.value)) continue;
          const ts = new Date(item.timestamp).toISOString().replace('T', ' ').slice(0, 16);
          if (!timeMap.has(ts)) timeMap.set(ts, new Map());
          timeMap.get(ts)!.set(item.value, item.count);
        }

        const headers = ['Time', ...values];
        const rows = [...timeMap.entries()].map(([ts, counts]) => [
          ts,
          ...values.map((v) => String(counts.get(v) ?? 0)),
        ]);

        return ok(
          `## ${groupBy} Timeline — Project ${projectId}\n\n` +
            `Bucket: ${result.bucket_size}\n\n` +
            table(headers, rows),
        );
      }),
  },

  // ── get_aggregated_buckets ─────────────────────────────────────
  {
    name: 'get_aggregated_buckets',
    description:
      'Get aggregated event counts in time buckets for a project. Flexible bucket sizes like "1 hour", "1 day".',
    inputSchema: {
      type: 'object',
      properties: {
        project_id: {
          type: 'number',
          description: 'The project ID',
        },
        start_date: {
          type: 'string',
          description: 'Start date in ISO 8601 format',
        },
        end_date: {
          type: 'string',
          description: 'End date in ISO 8601 format',
        },
        bucket_size: {
          type: 'string',
          description: 'Time bucket size (default "1 hour"). Examples: "1 hour", "1 day", "15 minutes".',
        },
        aggregation_level: {
          type: 'string',
          description: 'Count by: "events" (default), "sessions", or "visitors".',
          enum: ['events', 'sessions', 'visitors'],
        },
        environment_id: {
          type: 'number',
          description: 'Optional environment ID to filter by',
        },
      },
      required: ['project_id', 'start_date', 'end_date'],
    },
    handler: (args) =>
      handleToolCall(async () => {
        const client = getClient();
        const projectId = requireParam<number>(args, 'project_id');
        const startDate = requireParam<string>(args, 'start_date');
        const endDate = requireParam<string>(args, 'end_date');
        const bucketSize = optionalParam<string>(args, 'bucket_size');
        const aggLevel = optionalParam<string>(args, 'aggregation_level');
        const envId = optionalParam<number>(args, 'environment_id');

        const query: Record<string, unknown> = {
          start_date: startDate,
          end_date: endDate,
        };
        if (bucketSize !== undefined) query.bucket_size = bucketSize;
        if (aggLevel !== undefined) query.aggregation_level = aggLevel;
        if (envId !== undefined) query.environment_id = envId;

        const result = await client.get<AggregatedBucketsResponse>(
          `/projects/${projectId}/aggregated-buckets`,
          query,
        );

        if (!result.items.length) {
          return ok('No data found for this period.');
        }

        const rows = result.items.map((b) => [
          new Date(b.timestamp).toISOString().replace('T', ' ').slice(0, 16),
          b.count.toLocaleString(),
        ]);

        return ok(
          `## Aggregated Buckets — Project ${projectId}\n\n` +
            `**Total:** ${result.total.toLocaleString()} | **Bucket:** ${result.bucket_size} | **Level:** ${result.aggregation_level}\n\n` +
            table(['Time', 'Count'], rows),
        );
      }),
  },

  // ── get_general_stats ──────────────────────────────────────────
  {
    name: 'get_general_stats',
    description:
      'Get high-level analytics overview: total visitors, visits, page views, bounce rate, engagement rate, and trend percentages vs the previous period. Optionally includes per-project breakdown.',
    inputSchema: {
      type: 'object',
      properties: {
        start_date: {
          type: 'string',
          description: 'Start date in ISO 8601 format (e.g. "2026-02-17T00:00:00Z")',
        },
        end_date: {
          type: 'string',
          description: 'End date in ISO 8601 format',
        },
      },
      required: ['start_date', 'end_date'],
    },
    handler: (args) =>
      handleToolCall(async () => {
        const client = getClient();
        const startDate = requireParam<string>(args, 'start_date');
        const endDate = requireParam<string>(args, 'end_date');

        const stats = await client.get<GeneralStatsResponse>(
          '/analytics/general-stats',
          { start_date: startDate, end_date: endDate },
        );

        const trend = (val: number | null) =>
          val !== null && val !== undefined
            ? `${val >= 0 ? '+' : ''}${val.toFixed(1)}%`
            : 'N/A';

        let text =
          `## General Analytics Overview\n\n` +
          `| Metric | Value |\n| --- | --- |\n` +
          `| Unique Visitors | ${stats.total_unique_visitors.toLocaleString()} (${trend(stats.visitors_trend_percentage)} vs prev) |\n` +
          `| Total Visits | ${stats.total_visits.toLocaleString()} |\n` +
          `| Page Views | ${stats.total_page_views.toLocaleString()} (${trend(stats.page_views_trend_percentage)} vs prev) |\n` +
          `| Total Events | ${stats.total_events.toLocaleString()} |\n` +
          `| Avg Bounce Rate | ${stats.avg_bounce_rate.toFixed(1)}% |\n` +
          `| Avg Engagement Rate | ${stats.avg_engagement_rate.toFixed(1)}% |\n`;

        if (stats.project_breakdown.length > 0) {
          text += `\n### Per-Project Breakdown\n\n`;
          const rows = stats.project_breakdown.map((p) => [
            p.project_name ?? String(p.project_id),
            p.unique_visitors.toLocaleString(),
            p.total_page_views.toLocaleString(),
            `${p.bounce_rate.toFixed(1)}%`,
            `${p.engagement_rate.toFixed(1)}%`,
          ]);
          text += table(
            ['Project', 'Visitors', 'Page Views', 'Bounce Rate', 'Engagement'],
            rows,
          );
        }

        return ok(text);
      }),
  },

  // ── get_page_paths ─────────────────────────────────────────────
  {
    name: 'get_page_paths',
    description:
      'List the top pages for a project ranked by sessions or page views. Shows session count, page view count, and average time on page.',
    inputSchema: {
      type: 'object',
      properties: {
        project_id: {
          type: 'number',
          description: 'The project ID',
        },
        start_date: {
          type: 'string',
          description: 'Start date in ISO 8601 format',
        },
        end_date: {
          type: 'string',
          description: 'End date in ISO 8601 format',
        },
        environment_id: {
          type: 'number',
          description: 'Optional environment ID to filter by',
        },
        limit: {
          type: 'number',
          description: 'Max pages to return (default 100, max 1000)',
        },
      },
      required: ['project_id'],
    },
    handler: (args) =>
      handleToolCall(async () => {
        const client = getClient();
        const projectId = requireParam<number>(args, 'project_id');
        const startDate = optionalParam<string>(args, 'start_date');
        const endDate = optionalParam<string>(args, 'end_date');
        const envId = optionalParam<number>(args, 'environment_id');
        const limit = optionalParam<number>(args, 'limit');

        const query: Record<string, unknown> = { project_id: projectId };
        if (startDate !== undefined) query.start_date = startDate;
        if (endDate !== undefined) query.end_date = endDate;
        if (envId !== undefined) query.environment_id = envId;
        if (limit !== undefined) query.limit = limit;

        const result = await client.get<PagePathsResponse>(
          '/analytics/page-paths',
          query,
        );

        if (!result.page_paths.length) {
          return ok('No page data found.');
        }

        const fmtTime = (s: number | null) =>
          s !== null && s !== undefined ? `${s.toFixed(1)}s` : '—';

        const rows = result.page_paths.map((p) => [
          p.page_path,
          p.session_count.toLocaleString(),
          p.page_view_count.toLocaleString(),
          fmtTime(p.avg_time_seconds),
        ]);

        return ok(
          `## Top Pages — Project ${projectId}\n\n` +
            `**Total pages:** ${result.total_count}\n\n` +
            table(['Page', 'Sessions', 'Views', 'Avg Time'], rows),
        );
      }),
  },

  // ── get_page_detail ────────────────────────────────────────────
  {
    name: 'get_page_detail',
    description:
      'Get detailed analytics for a specific page: bounce rate, entry/exit rate, average time, activity over time, top countries, and referrers.',
    inputSchema: {
      type: 'object',
      properties: {
        project_id: {
          type: 'number',
          description: 'The project ID',
        },
        page_path: {
          type: 'string',
          description: 'The page path to analyze (e.g. "/", "/pricing", "/docs/getting-started")',
        },
        start_date: {
          type: 'string',
          description: 'Start date in ISO 8601 format',
        },
        end_date: {
          type: 'string',
          description: 'End date in ISO 8601 format',
        },
        environment_id: {
          type: 'number',
          description: 'Optional environment ID to filter by',
        },
        bucket_interval: {
          type: 'string',
          description: 'Time bucket for activity chart: "hour", "day", "week", or "month" (auto if omitted)',
          enum: ['hour', 'day', 'week', 'month'],
        },
      },
      required: ['project_id', 'page_path', 'start_date', 'end_date'],
    },
    handler: (args) =>
      handleToolCall(async () => {
        const client = getClient();
        const projectId = requireParam<number>(args, 'project_id');
        const pagePath = requireParam<string>(args, 'page_path');
        const startDate = requireParam<string>(args, 'start_date');
        const endDate = requireParam<string>(args, 'end_date');
        const envId = optionalParam<number>(args, 'environment_id');
        const bucket = optionalParam<string>(args, 'bucket_interval');

        const query: Record<string, unknown> = {
          project_id: projectId,
          page_path: pagePath,
          start_date: startDate,
          end_date: endDate,
        };
        if (envId !== undefined) query.environment_id = envId;
        if (bucket !== undefined) query.bucket_interval = bucket;

        const detail = await client.get<PagePathDetailResponse>(
          '/analytics/page-path-detail',
          query,
        );

        let text =
          `## Page Detail: ${detail.page_path}\n\n` +
          `| Metric | Value |\n| --- | --- |\n` +
          `| Unique Visitors | ${detail.unique_visitors.toLocaleString()} |\n` +
          `| Total Page Views | ${detail.total_page_views.toLocaleString()} |\n` +
          `| Avg Time on Page | ${detail.avg_time_on_page.toFixed(1)}s |\n` +
          `| Bounce Rate | ${detail.bounce_rate.toFixed(1)}% |\n` +
          `| Entry Rate | ${detail.entry_rate.toFixed(1)}% |\n` +
          `| Exit Rate | ${detail.exit_rate.toFixed(1)}% |\n`;

        if (detail.referrers.length > 0) {
          text += `\n### Top Referrers\n\n`;
          const refRows = detail.referrers.slice(0, 10).map((r) => [
            r.referrer || '(direct)',
            r.visits.toLocaleString(),
            `${r.percentage.toFixed(1)}%`,
          ]);
          text += table(['Referrer', 'Visits', '%'], refRows);
        }

        if (detail.countries.length > 0) {
          text += `\n\n### Top Countries\n\n`;
          const countryRows = detail.countries.slice(0, 10).map((c) => [
            `${c.country_code ?? ''} ${c.country}`.trim(),
            c.visitors.toLocaleString(),
            c.page_views.toLocaleString(),
            `${c.percentage.toFixed(1)}%`,
          ]);
          text += table(['Country', 'Visitors', 'Views', '%'], countryRows);
        }

        return ok(text);
      }),
  },

  // ── get_page_flow ──────────────────────────────────────────────
  {
    name: 'get_page_flow',
    description:
      'Analyze user navigation flow: top entry pages, exit pages, drop-off points (with bounce rates), and page-to-page transitions.',
    inputSchema: {
      type: 'object',
      properties: {
        project_id: {
          type: 'number',
          description: 'The project ID',
        },
        start_date: {
          type: 'string',
          description: 'Start date in ISO 8601 format',
        },
        end_date: {
          type: 'string',
          description: 'End date in ISO 8601 format',
        },
        environment_id: {
          type: 'number',
          description: 'Optional environment ID to filter by',
        },
        limit: {
          type: 'number',
          description: 'Max entry/exit pages (default 20, max 100)',
        },
        transitions_limit: {
          type: 'number',
          description: 'Max transitions to return (default 50, max 200)',
        },
      },
      required: ['project_id', 'start_date', 'end_date'],
    },
    handler: (args) =>
      handleToolCall(async () => {
        const client = getClient();
        const projectId = requireParam<number>(args, 'project_id');
        const startDate = requireParam<string>(args, 'start_date');
        const endDate = requireParam<string>(args, 'end_date');
        const envId = optionalParam<number>(args, 'environment_id');
        const limit = optionalParam<number>(args, 'limit');
        const transLimit = optionalParam<number>(args, 'transitions_limit');

        const query: Record<string, unknown> = {
          project_id: projectId,
          start_date: startDate,
          end_date: endDate,
        };
        if (envId !== undefined) query.environment_id = envId;
        if (limit !== undefined) query.limit = limit;
        if (transLimit !== undefined) query.transitions_limit = transLimit;

        const flow = await client.get<PageFlowResponse>(
          '/analytics/page-flow',
          query,
        );

        let text =
          `## Page Flow — Project ${projectId}\n\n` +
          `**Total pages:** ${flow.total_pages} | **Total sessions:** ${flow.total_sessions.toLocaleString()}\n\n`;

        // Entry pages
        if (flow.top_entry_pages.length > 0) {
          text += `### Top Entry Pages\n\n`;
          const entryRows = flow.top_entry_pages.map((p) => [
            p.page_path,
            p.count.toLocaleString(),
            `${p.percentage.toFixed(1)}%`,
            p.bounce_rate !== undefined ? `${p.bounce_rate.toFixed(1)}%` : '—',
          ]);
          text += table(['Page', 'Entries', '%', 'Bounce Rate'], entryRows);
        }

        // Exit pages
        if (flow.top_exit_pages.length > 0) {
          text += `\n\n### Top Exit Pages\n\n`;
          const exitRows = flow.top_exit_pages.map((p) => [
            p.page_path,
            p.count.toLocaleString(),
            `${p.percentage.toFixed(1)}%`,
          ]);
          text += table(['Page', 'Exits', '%'], exitRows);
        }

        // Drop-off points
        if (flow.drop_off_points.length > 0) {
          text += `\n\n### Drop-off Points\n\n`;
          const dropRows = flow.drop_off_points.map((d) => [
            d.page_path,
            d.views.toLocaleString(),
            d.drop_off_count.toLocaleString(),
            `${d.drop_off_rate.toFixed(1)}%`,
          ]);
          text += table(['Page', 'Views', 'Drop-offs', 'Drop-off Rate'], dropRows);
        }

        // Top transitions
        if (flow.transitions.length > 0) {
          text += `\n\n### Top Transitions\n\n`;
          const transRows = flow.transitions.slice(0, 20).map((t) => [
            t.from_path,
            t.to_path,
            t.count.toLocaleString(),
          ]);
          text += table(['From', 'To', 'Count'], transRows);
        }

        return ok(text);
      }),
  },
];
