import { getClient } from '../api/index.js';
import {
  ok,
  json,
  table,
  formatDate,
  handleToolCall,
  requireParam,
  optionalParam,
} from './_helpers.js';
import type { ToolDefinition } from '../types/index.js';

interface ProxyLog {
  id: number;
  method: string;
  host: string;
  path: string;
  status_code: number;
  response_time_ms?: number;
  project_id?: number;
  environment_id?: number;
  created_at?: string;
}

interface ProxyLogListResponse {
  logs: ProxyLog[];
  total: number;
  page: number;
  total_pages: number;
}

interface ProxyLogStats {
  stats: Array<Record<string, unknown>>;
}

interface ProxyLogTodayStats {
  date: string;
  total_requests: number;
}

export const tools: ToolDefinition[] = [
  {
    name: 'list_proxy_logs',
    description: 'List proxy logs with optional filtering and pagination',
    inputSchema: {
      type: 'object',
      properties: {
        page: {
          type: 'number',
          description: 'Page number (default: 1)',
        },
        page_size: {
          type: 'number',
          description: 'Items per page (default: 20, max: 100)',
        },
        project_id: {
          type: 'number',
          description: 'Filter by project ID',
        },
        environment_id: {
          type: 'number',
          description: 'Filter by environment ID',
        },
        method: {
          type: 'string',
          description: 'Filter by HTTP method (e.g. GET, POST)',
        },
        status_code: {
          type: 'number',
          description: 'Filter by HTTP status code',
        },
        host: {
          type: 'string',
          description: 'Filter by host',
        },
        path: {
          type: 'string',
          description: 'Filter by path',
        },
        start_date: {
          type: 'string',
          description: 'Start date filter (ISO 8601)',
        },
        end_date: {
          type: 'string',
          description: 'End date filter (ISO 8601)',
        },
        sort_by: {
          type: 'string',
          description: 'Field to sort by',
        },
        sort_order: {
          type: 'string',
          description: 'Sort order (asc or desc)',
        },
      },
    },
    handler: (args) =>
      handleToolCall(async () => {
        const client = getClient();

        const query: Record<string, unknown> = {};
        const page = optionalParam<number>(args, 'page');
        const pageSize = optionalParam<number>(args, 'page_size');
        const projectId = optionalParam<number>(args, 'project_id');
        const environmentId = optionalParam<number>(args, 'environment_id');
        const method = optionalParam<string>(args, 'method');
        const statusCode = optionalParam<number>(args, 'status_code');
        const host = optionalParam<string>(args, 'host');
        const path = optionalParam<string>(args, 'path');
        const startDate = optionalParam<string>(args, 'start_date');
        const endDate = optionalParam<string>(args, 'end_date');
        const sortBy = optionalParam<string>(args, 'sort_by');
        const sortOrder = optionalParam<string>(args, 'sort_order');

        if (page !== undefined) query.page = page;
        if (pageSize !== undefined) query.page_size = pageSize;
        if (projectId !== undefined) query.project_id = projectId;
        if (environmentId !== undefined) query.environment_id = environmentId;
        if (method !== undefined) query.method = method;
        if (statusCode !== undefined) query.status_code = statusCode;
        if (host !== undefined) query.host = host;
        if (path !== undefined) query.path = path;
        if (startDate !== undefined) query.start_date = startDate;
        if (endDate !== undefined) query.end_date = endDate;
        if (sortBy !== undefined) query.sort_by = sortBy;
        if (sortOrder !== undefined) query.sort_order = sortOrder;

        const data = await client.get<ProxyLogListResponse>(
          '/proxy-logs',
          query
        );
        const logs = data.logs ?? [];

        if (logs.length === 0) {
          return ok('No proxy logs found.');
        }

        const rows = logs.map((l) => [
          String(l.id),
          l.method ?? '',
          String(l.status_code ?? ''),
          l.host ?? '',
          l.path ?? '',
          l.response_time_ms !== undefined ? `${l.response_time_ms}ms` : 'N/A',
          formatDate(l.created_at),
        ]);

        return ok(
          `## Proxy Logs (page ${data.page}/${data.total_pages}, total: ${data.total})\n\n` +
            table(
              ['ID', 'Method', 'Status', 'Host', 'Path', 'Response Time', 'Created'],
              rows
            )
        );
      }),
  },

  {
    name: 'get_proxy_log',
    description: 'Get details of a specific proxy log entry by ID',
    inputSchema: {
      type: 'object',
      properties: {
        id: {
          type: 'number',
          description: 'Proxy log ID',
        },
      },
      required: ['id'],
    },
    handler: (args) =>
      handleToolCall(async () => {
        const client = getClient();
        const id = requireParam<number>(args, 'id');
        const data = await client.get<Record<string, unknown>>(
          `/proxy-logs/${id}`
        );
        return json('Proxy Log Details', data);
      }),
  },

  {
    name: 'get_proxy_log_by_request_id',
    description: 'Get a proxy log entry by its request ID',
    inputSchema: {
      type: 'object',
      properties: {
        request_id: {
          type: 'string',
          description: 'The unique request ID',
        },
      },
      required: ['request_id'],
    },
    handler: (args) =>
      handleToolCall(async () => {
        const client = getClient();
        const requestId = requireParam<string>(args, 'request_id');
        const data = await client.get<Record<string, unknown>>(
          `/proxy-logs/request/${requestId}`
        );
        return json('Proxy Log (by Request ID)', data);
      }),
  },

  {
    name: 'get_proxy_log_stats',
    description:
      'Get aggregated proxy log statistics for a time range, optionally bucketed by interval',
    inputSchema: {
      type: 'object',
      properties: {
        start_time: {
          type: 'string',
          description: 'Start time (ISO 8601)',
        },
        end_time: {
          type: 'string',
          description: 'End time (ISO 8601)',
        },
        bucket_interval: {
          type: 'string',
          description: 'Bucket interval (e.g. "1h", "15m", "1d")',
        },
      },
      required: ['start_time', 'end_time'],
    },
    handler: (args) =>
      handleToolCall(async () => {
        const client = getClient();
        const startTime = requireParam<string>(args, 'start_time');
        const endTime = requireParam<string>(args, 'end_time');
        const bucketInterval = optionalParam<string>(args, 'bucket_interval');

        const query: Record<string, unknown> = {
          start_time: startTime,
          end_time: endTime,
        };
        if (bucketInterval !== undefined) query.bucket_interval = bucketInterval;

        const data = await client.get<ProxyLogStats>(
          '/proxy-logs/stats/time-buckets',
          query
        );
        return json('Proxy Log Stats', data);
      }),
  },

  {
    name: 'get_proxy_log_today_stats',
    description: 'Get a summary of proxy log statistics for today',
    inputSchema: {
      type: 'object',
      properties: {},
    },
    handler: (args) =>
      handleToolCall(async () => {
        const client = getClient();
        const data = await client.get<ProxyLogTodayStats>(
          '/proxy-logs/stats/today'
        );
        return ok(
          `## Today's Proxy Stats\n\n- **Date:** ${data.date}\n- **Total Requests:** ${data.total_requests}`
        );
      }),
  },
];
