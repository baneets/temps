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

interface ErrorGroup {
  id: number;
  project_id: number;
  title: string;
  status: string;
  level?: string;
  event_count?: number;
  first_seen?: string;
  last_seen?: string;
  assigned_to?: string;
  environment_id?: number;
  created_at?: string;
  updated_at?: string;
}

interface ErrorEvent {
  id: number;
  group_id: number;
  message?: string;
  timestamp?: string;
  environment?: string;
  release?: string;
  stack_trace?: string;
  context?: Record<string, unknown>;
}

interface Pagination {
  page: number;
  page_size: number;
  total: number;
  total_pages: number;
}

interface PaginatedErrorGroups {
  data: ErrorGroup[];
  pagination: Pagination;
}

interface PaginatedErrorEvents {
  data: ErrorEvent[];
  pagination: Pagination;
}

interface ErrorStats {
  total_groups?: number;
  total_events?: number;
  unresolved_count?: number;
  resolved_count?: number;
  ignored_count?: number;
  [key: string]: unknown;
}

interface ErrorDashboard {
  [key: string]: unknown;
}

export const tools: ToolDefinition[] = [
  {
    name: 'list_error_groups',
    description:
      'List error groups for a project with optional filtering and pagination',
    inputSchema: {
      type: 'object',
      properties: {
        project_id: {
          type: 'number',
          description: 'Project ID',
        },
        page: {
          type: 'number',
          description: 'Page number (default: 1)',
        },
        page_size: {
          type: 'number',
          description: 'Items per page (default: 20, max: 100)',
        },
        status: {
          type: 'string',
          description:
            'Filter by status (e.g. unresolved, resolved, ignored)',
        },
        environment_id: {
          type: 'number',
          description: 'Filter by environment ID',
        },
        start_date: {
          type: 'string',
          description: 'Filter errors after this date (ISO 8601)',
        },
        end_date: {
          type: 'string',
          description: 'Filter errors before this date (ISO 8601)',
        },
        sort_by: {
          type: 'string',
          description: 'Field to sort by (e.g. last_seen, event_count)',
        },
        sort_order: {
          type: 'string',
          description: 'Sort order: asc or desc',
          enum: ['asc', 'desc'],
        },
      },
      required: ['project_id'],
    },
    handler: (args) =>
      handleToolCall(async () => {
        const projectId = requireParam<number>(args, 'project_id');
        const page = optionalParam<number>(args, 'page');
        const pageSize = optionalParam<number>(args, 'page_size');
        const status = optionalParam<string>(args, 'status');
        const environmentId = optionalParam<number>(args, 'environment_id');
        const startDate = optionalParam<string>(args, 'start_date');
        const endDate = optionalParam<string>(args, 'end_date');
        const sortBy = optionalParam<string>(args, 'sort_by');
        const sortOrder = optionalParam<string>(args, 'sort_order');
        const client = getClient();

        const query: Record<string, unknown> = {};
        if (page !== undefined) query.page = page;
        if (pageSize !== undefined) query.page_size = pageSize;
        if (status !== undefined) query.status = status;
        if (environmentId !== undefined) query.environment_id = environmentId;
        if (startDate !== undefined) query.start_date = startDate;
        if (endDate !== undefined) query.end_date = endDate;
        if (sortBy !== undefined) query.sort_by = sortBy;
        if (sortOrder !== undefined) query.sort_order = sortOrder;

        const response = await client.get<PaginatedErrorGroups>(
          `/projects/${projectId}/error-groups`,
          query
        );

        const groups = response.data;
        if (!groups || groups.length === 0) {
          return ok('No error groups found.');
        }

        const rows = groups.map((g) => [
          String(g.id),
          g.title,
          g.status,
          g.level ?? 'N/A',
          String(g.event_count ?? 0),
          formatDate(g.last_seen),
        ]);

        const header = `Page ${response.pagination.page}/${response.pagination.total_pages} (${response.pagination.total} total)\n\n`;

        return ok(
          header +
            table(
              ['ID', 'Title', 'Status', 'Level', 'Events', 'Last Seen'],
              rows
            )
        );
      }),
  },
  {
    name: 'get_error_group',
    description: 'Get details of a specific error group',
    inputSchema: {
      type: 'object',
      properties: {
        project_id: {
          type: 'number',
          description: 'Project ID',
        },
        group_id: {
          type: 'number',
          description: 'Error group ID',
        },
      },
      required: ['project_id', 'group_id'],
    },
    handler: (args) =>
      handleToolCall(async () => {
        const projectId = requireParam<number>(args, 'project_id');
        const groupId = requireParam<number>(args, 'group_id');
        const client = getClient();
        const group = await client.get<ErrorGroup>(
          `/projects/${projectId}/error-groups/${groupId}`
        );

        return json('Error Group Details', group);
      }),
  },
  {
    name: 'update_error_group',
    description:
      'Update an error group status or assignment',
    inputSchema: {
      type: 'object',
      properties: {
        project_id: {
          type: 'number',
          description: 'Project ID',
        },
        group_id: {
          type: 'number',
          description: 'Error group ID',
        },
        status: {
          type: 'string',
          description:
            'New status (e.g. unresolved, resolved, ignored)',
        },
        assigned_to: {
          type: 'string',
          description: 'User to assign the error group to',
        },
      },
      required: ['project_id', 'group_id', 'status'],
    },
    handler: (args) =>
      handleToolCall(async () => {
        const projectId = requireParam<number>(args, 'project_id');
        const groupId = requireParam<number>(args, 'group_id');
        const status = requireParam<string>(args, 'status');
        const assignedTo = optionalParam<string>(args, 'assigned_to');
        const client = getClient();

        const body: Record<string, unknown> = { status };
        if (assignedTo !== undefined) body.assigned_to = assignedTo;

        const group = await client.put<ErrorGroup>(
          `/projects/${projectId}/error-groups/${groupId}`,
          body
        );

        return json('Error Group Updated', group);
      }),
  },
  {
    name: 'list_error_events',
    description: 'List individual error events within an error group',
    inputSchema: {
      type: 'object',
      properties: {
        project_id: {
          type: 'number',
          description: 'Project ID',
        },
        group_id: {
          type: 'number',
          description: 'Error group ID',
        },
        page: {
          type: 'number',
          description: 'Page number (default: 1)',
        },
        page_size: {
          type: 'number',
          description: 'Items per page (default: 20, max: 100)',
        },
      },
      required: ['project_id', 'group_id'],
    },
    handler: (args) =>
      handleToolCall(async () => {
        const projectId = requireParam<number>(args, 'project_id');
        const groupId = requireParam<number>(args, 'group_id');
        const page = optionalParam<number>(args, 'page');
        const pageSize = optionalParam<number>(args, 'page_size');
        const client = getClient();

        const query: Record<string, unknown> = {};
        if (page !== undefined) query.page = page;
        if (pageSize !== undefined) query.page_size = pageSize;

        const response = await client.get<PaginatedErrorEvents>(
          `/projects/${projectId}/error-groups/${groupId}/events`,
          query
        );

        const events = response.data;
        if (!events || events.length === 0) {
          return ok('No error events found for this group.');
        }

        const rows = events.map((e) => [
          String(e.id),
          e.message ?? 'N/A',
          e.environment ?? 'N/A',
          e.release ?? 'N/A',
          formatDate(e.timestamp),
        ]);

        const header = `Page ${response.pagination.page}/${response.pagination.total_pages} (${response.pagination.total} total)\n\n`;

        return ok(
          header +
            table(
              ['ID', 'Message', 'Environment', 'Release', 'Timestamp'],
              rows
            )
        );
      }),
  },
  {
    name: 'get_error_event',
    description: 'Get full details of a specific error event including stack trace',
    inputSchema: {
      type: 'object',
      properties: {
        project_id: {
          type: 'number',
          description: 'Project ID',
        },
        group_id: {
          type: 'number',
          description: 'Error group ID',
        },
        event_id: {
          type: 'number',
          description: 'Error event ID',
        },
      },
      required: ['project_id', 'group_id', 'event_id'],
    },
    handler: (args) =>
      handleToolCall(async () => {
        const projectId = requireParam<number>(args, 'project_id');
        const groupId = requireParam<number>(args, 'group_id');
        const eventId = requireParam<number>(args, 'event_id');
        const client = getClient();

        const event = await client.get<ErrorEvent>(
          `/projects/${projectId}/error-groups/${groupId}/events/${eventId}`
        );

        return json('Error Event Details', event);
      }),
  },
  {
    name: 'get_error_stats',
    description:
      'Get error statistics summary for a project',
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
        const stats = await client.get<ErrorStats>(
          `/projects/${projectId}/error-stats`
        );

        return json('Error Stats', stats);
      }),
  },
  {
    name: 'get_error_dashboard',
    description:
      'Get error tracking dashboard data with time-range comparison',
    inputSchema: {
      type: 'object',
      properties: {
        project_id: {
          type: 'number',
          description: 'Project ID',
        },
        start_time: {
          type: 'string',
          description: 'Start time for the dashboard period (ISO 8601)',
        },
        end_time: {
          type: 'string',
          description: 'End time for the dashboard period (ISO 8601)',
        },
        compare_to_previous: {
          type: 'boolean',
          description:
            'Whether to include comparison with the previous period',
        },
      },
      required: ['project_id', 'start_time', 'end_time'],
    },
    handler: (args) =>
      handleToolCall(async () => {
        const projectId = requireParam<number>(args, 'project_id');
        const startTime = requireParam<string>(args, 'start_time');
        const endTime = requireParam<string>(args, 'end_time');
        const compareToPrevious = optionalParam<boolean>(
          args,
          'compare_to_previous'
        );
        const client = getClient();

        const query: Record<string, unknown> = {
          start_time: startTime,
          end_time: endTime,
        };
        if (compareToPrevious !== undefined)
          query.compare_to_previous = compareToPrevious;

        const dashboard = await client.get<ErrorDashboard>(
          `/projects/${projectId}/error-dashboard-stats`,
          query
        );

        return json('Error Dashboard', dashboard);
      }),
  },
];
