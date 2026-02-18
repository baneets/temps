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

interface Monitor {
  id: number;
  name: string;
  monitor_type: string;
  status: string;
  check_interval_seconds: number;
  environment_id: number;
  created_at?: string;
  updated_at?: string;
}

interface MonitorStatus {
  monitor_id: number;
  status: string;
  uptime_percentage?: number;
  average_response_time_ms?: number;
  last_checked_at?: string;
}

interface MonitorHistory {
  entries?: Array<{
    timestamp: string;
    status: string;
    response_time_ms?: number;
  }>;
}

export const tools: ToolDefinition[] = [
  {
    name: 'list_monitors',
    description: 'List all monitors for a project',
    inputSchema: {
      type: 'object',
      properties: {
        project_id: {
          type: 'number',
          description: 'Project ID',
        },
      },
      required: ['project_id'],
    },
    handler: (args) =>
      handleToolCall(async () => {
        const projectId = requireParam<number>(args, 'project_id');
        const client = getClient();
        const monitors = await client.get<Monitor[]>(
          `/projects/${projectId}/monitors`
        );

        if (!monitors || monitors.length === 0) {
          return ok('No monitors found for this project.');
        }

        const rows = monitors.map((m) => [
          String(m.id),
          m.name,
          m.monitor_type,
          m.status,
        ]);

        return ok(table(['ID', 'Name', 'Type', 'Status'], rows));
      }),
  },
  {
    name: 'create_monitor',
    description: 'Create a new monitor for a project',
    inputSchema: {
      type: 'object',
      properties: {
        project_id: {
          type: 'number',
          description: 'Project ID',
        },
        name: {
          type: 'string',
          description: 'Monitor name',
        },
        monitor_type: {
          type: 'string',
          description: 'Monitor type (e.g. http, tcp, ping)',
        },
        check_interval_seconds: {
          type: 'number',
          description: 'Check interval in seconds',
        },
        environment_id: {
          type: 'number',
          description: 'Environment ID to monitor',
        },
      },
      required: [
        'project_id',
        'name',
        'monitor_type',
        'check_interval_seconds',
        'environment_id',
      ],
    },
    handler: (args) =>
      handleToolCall(async () => {
        const projectId = requireParam<number>(args, 'project_id');
        const name = requireParam<string>(args, 'name');
        const monitorType = requireParam<string>(args, 'monitor_type');
        const checkIntervalSeconds = requireParam<number>(
          args,
          'check_interval_seconds'
        );
        const environmentId = requireParam<number>(args, 'environment_id');
        const client = getClient();

        const monitor = await client.post<Monitor>(
          `/projects/${projectId}/monitors`,
          {
            name,
            monitor_type: monitorType,
            check_interval_seconds: checkIntervalSeconds,
            environment_id: environmentId,
          }
        );

        return json('Monitor Created', monitor);
      }),
  },
  {
    name: 'get_monitor',
    description: 'Get details of a specific monitor',
    inputSchema: {
      type: 'object',
      properties: {
        monitor_id: {
          type: 'number',
          description: 'Monitor ID',
        },
      },
      required: ['monitor_id'],
    },
    handler: (args) =>
      handleToolCall(async () => {
        const monitorId = requireParam<number>(args, 'monitor_id');
        const client = getClient();
        const monitor = await client.get<Monitor>(
          `/monitors/${monitorId}`
        );

        return json('Monitor Details', monitor);
      }),
  },
  {
    name: 'delete_monitor',
    description: 'Delete a monitor',
    inputSchema: {
      type: 'object',
      properties: {
        monitor_id: {
          type: 'number',
          description: 'Monitor ID',
        },
      },
      required: ['monitor_id'],
    },
    handler: (args) =>
      handleToolCall(async () => {
        const monitorId = requireParam<number>(args, 'monitor_id');
        const client = getClient();
        await client.delete(`/monitors/${monitorId}`);

        return ok(`Monitor ${monitorId} deleted successfully.`);
      }),
  },
  {
    name: 'get_monitor_status',
    description:
      'Get current status of a monitor including uptime and response time',
    inputSchema: {
      type: 'object',
      properties: {
        monitor_id: {
          type: 'number',
          description: 'Monitor ID',
        },
        start_time: {
          type: 'string',
          description: 'Start time (ISO 8601). Defaults to 24 hours ago.',
        },
        end_time: {
          type: 'string',
          description: 'End time (ISO 8601). Defaults to now.',
        },
      },
      required: ['monitor_id'],
    },
    handler: (args) =>
      handleToolCall(async () => {
        const monitorId = requireParam<number>(args, 'monitor_id');
        const startTime = optionalParam<string>(args, 'start_time') ??
          new Date(Date.now() - 24 * 60 * 60 * 1000).toISOString();
        const endTime = optionalParam<string>(args, 'end_time');
        const client = getClient();

        const query: Record<string, unknown> = {
          start_time: startTime,
          end_time: endTime ?? new Date().toISOString(),
        };

        const status = await client.get<MonitorStatus>(
          `/monitors/${monitorId}/current-status`,
          query
        );

        return json('Monitor Status', status);
      }),
  },
  {
    name: 'get_monitor_history',
    description: 'Get uptime history for a monitor',
    inputSchema: {
      type: 'object',
      properties: {
        monitor_id: {
          type: 'number',
          description: 'Monitor ID',
        },
        days: {
          type: 'number',
          description: 'Number of days of history to retrieve',
        },
        start_time: {
          type: 'string',
          description: 'Start time (ISO 8601)',
        },
        end_time: {
          type: 'string',
          description: 'End time (ISO 8601)',
        },
      },
      required: ['monitor_id'],
    },
    handler: (args) =>
      handleToolCall(async () => {
        const monitorId = requireParam<number>(args, 'monitor_id');
        const days = optionalParam<number>(args, 'days');
        const startTime = optionalParam<string>(args, 'start_time');
        const endTime = optionalParam<string>(args, 'end_time');
        const client = getClient();

        const effectiveStartTime = startTime ??
          new Date(Date.now() - (days ?? 1) * 24 * 60 * 60 * 1000).toISOString();
        const query: Record<string, unknown> = {
          start_time: effectiveStartTime,
          end_time: endTime ?? new Date().toISOString(),
        };

        const history = await client.get<MonitorHistory>(
          `/monitors/${monitorId}/uptime`,
          query
        );

        return json('Monitor History', history);
      }),
  },
];
