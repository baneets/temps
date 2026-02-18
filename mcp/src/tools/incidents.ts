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

interface Incident {
  id: number;
  title: string;
  severity: string;
  status: string;
  description?: string;
  project_id: number;
  environment_id?: number;
  created_at?: string;
  updated_at?: string;
}

interface IncidentUpdate {
  id: number;
  incident_id: number;
  status: string;
  message: string;
  created_at?: string;
}

export const tools: ToolDefinition[] = [
  {
    name: 'list_incidents',
    description: 'List incidents for a project with optional filtering',
    inputSchema: {
      type: 'object',
      properties: {
        project_id: {
          type: 'number',
          description: 'Project ID',
        },
        status: {
          type: 'string',
          description: 'Filter by incident status (e.g. open, resolved)',
        },
        environment_id: {
          type: 'number',
          description: 'Filter by environment ID',
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
      required: ['project_id'],
    },
    handler: (args) =>
      handleToolCall(async () => {
        const client = getClient();
        const projectId = requireParam<number>(args, 'project_id');
        const status = optionalParam<string>(args, 'status');
        const environmentId = optionalParam<number>(args, 'environment_id');
        const page = optionalParam<number>(args, 'page');
        const pageSize = optionalParam<number>(args, 'page_size');

        const query: Record<string, unknown> = {};
        if (status !== undefined) query.status = status;
        if (environmentId !== undefined) query.environment_id = environmentId;
        if (page !== undefined) query.page = page;
        if (pageSize !== undefined) query.page_size = pageSize;

        const data = await client.get<Incident[] | { incidents: Incident[] }>(
          `/projects/${projectId}/incidents`,
          query
        );

        const incidents = Array.isArray(data) ? data : (data?.incidents ?? []);

        if (!incidents || incidents.length === 0) {
          return ok('No incidents found for this project.');
        }

        const rows = incidents.map((i: Incident) => [
          String(i.id),
          i.title ?? '',
          i.severity ?? '',
          i.status ?? '',
          formatDate(i.created_at),
        ]);

        return ok(
          `## Incidents for Project ${projectId}\n\n${table(
            ['ID', 'Title', 'Severity', 'Status', 'Created'],
            rows
          )}`
        );
      }),
  },

  {
    name: 'create_incident',
    description: 'Create a new incident for a project',
    inputSchema: {
      type: 'object',
      properties: {
        project_id: {
          type: 'number',
          description: 'Project ID',
        },
        title: {
          type: 'string',
          description: 'Incident title',
        },
        severity: {
          type: 'string',
          description: 'Incident severity (e.g. critical, major, minor)',
        },
        description: {
          type: 'string',
          description: 'Optional description of the incident',
        },
      },
      required: ['project_id', 'title', 'severity'],
    },
    handler: (args) =>
      handleToolCall(async () => {
        const client = getClient();
        const projectId = requireParam<number>(args, 'project_id');
        const title = requireParam<string>(args, 'title');
        const severity = requireParam<string>(args, 'severity');
        const description = optionalParam<string>(args, 'description');

        const body: Record<string, unknown> = { title, severity };
        if (description !== undefined) body.description = description;

        const incident = await client.post<Incident>(
          `/projects/${projectId}/incidents`,
          body
        );
        return json('Incident Created', incident);
      }),
  },

  {
    name: 'get_incident',
    description: 'Get details of a specific incident',
    inputSchema: {
      type: 'object',
      properties: {
        incident_id: {
          type: 'number',
          description: 'Incident ID',
        },
      },
      required: ['incident_id'],
    },
    handler: (args) =>
      handleToolCall(async () => {
        const client = getClient();
        const incidentId = requireParam<number>(args, 'incident_id');
        const incident = await client.get<Incident>(
          `/incidents/${incidentId}`
        );
        return json('Incident Details', incident);
      }),
  },

  {
    name: 'update_incident_status',
    description: 'Update the status of an incident with a status message',
    inputSchema: {
      type: 'object',
      properties: {
        incident_id: {
          type: 'number',
          description: 'Incident ID',
        },
        status: {
          type: 'string',
          description: 'New status (e.g. investigating, identified, monitoring, resolved)',
        },
        message: {
          type: 'string',
          description: 'Status update message',
        },
      },
      required: ['incident_id', 'status', 'message'],
    },
    handler: (args) =>
      handleToolCall(async () => {
        const client = getClient();
        const incidentId = requireParam<number>(args, 'incident_id');
        const status = requireParam<string>(args, 'status');
        const message = requireParam<string>(args, 'message');

        const incident = await client.patch<Incident>(
          `/incidents/${incidentId}/status`,
          { status, message }
        );
        return json('Incident Status Updated', incident);
      }),
  },

  {
    name: 'get_incident_updates',
    description: 'Get the status update history for an incident',
    inputSchema: {
      type: 'object',
      properties: {
        incident_id: {
          type: 'number',
          description: 'Incident ID',
        },
      },
      required: ['incident_id'],
    },
    handler: (args) =>
      handleToolCall(async () => {
        const client = getClient();
        const incidentId = requireParam<number>(args, 'incident_id');
        const updates = await client.get<IncidentUpdate[]>(
          `/incidents/${incidentId}/updates`
        );

        if (!updates || updates.length === 0) {
          return ok(`No updates found for incident ${incidentId}.`);
        }

        const rows = updates.map((u) => [
          String(u.id),
          u.status ?? '',
          u.message ?? '',
          formatDate(u.created_at),
        ]);

        return ok(
          `## Updates for Incident ${incidentId}\n\n${table(
            ['ID', 'Status', 'Message', 'Created'],
            rows
          )}`
        );
      }),
  },

  {
    name: 'get_bucketed_incidents',
    description:
      'Get incidents aggregated into time buckets for a project',
    inputSchema: {
      type: 'object',
      properties: {
        project_id: {
          type: 'number',
          description: 'Project ID',
        },
        interval: {
          type: 'string',
          description: 'Bucket interval (e.g. "1h", "1d")',
        },
        start_time: {
          type: 'string',
          description: 'Start time (ISO 8601)',
        },
        end_time: {
          type: 'string',
          description: 'End time (ISO 8601)',
        },
        environment_id: {
          type: 'number',
          description: 'Optional environment ID filter',
        },
      },
      required: ['project_id'],
    },
    handler: (args) =>
      handleToolCall(async () => {
        const client = getClient();
        const projectId = requireParam<number>(args, 'project_id');
        const interval = optionalParam<string>(args, 'interval');
        const startTime = optionalParam<string>(args, 'start_time');
        const endTime = optionalParam<string>(args, 'end_time');
        const environmentId = optionalParam<number>(args, 'environment_id');

        const effectiveStartTime = startTime ??
          new Date(Date.now() - 7 * 24 * 60 * 60 * 1000).toISOString();
        const query: Record<string, unknown> = {
          start_time: effectiveStartTime,
          end_time: endTime ?? new Date().toISOString(),
        };
        if (interval !== undefined) query.interval = interval;
        if (environmentId !== undefined) query.environment_id = environmentId;

        const data = await client.get<Record<string, unknown>>(
          `/projects/${projectId}/incidents/bucketed`,
          query
        );
        return json('Bucketed Incidents', data);
      }),
  },
];
