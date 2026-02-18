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

interface Container {
  id: string;
  name: string;
  image: string;
  status: string;
  created_at?: string;
}

interface ContainerListResponse {
  containers: Container[];
  total: number;
}

interface ContainerDetail {
  id: string;
  name: string;
  image: string;
  status: string;
  ports?: Array<{ host: number; container: number; protocol?: string }>;
  environment?: Record<string, string>;
  created_at?: string;
  started_at?: string;
}

interface ContainerMetrics {
  container_id: string;
  cpu_usage_percent?: number;
  memory_usage_bytes?: number;
  memory_limit_bytes?: number;
  network_rx_bytes?: number;
  network_tx_bytes?: number;
  timestamp?: string;
}

function envBasePath(
  projectId: number,
  environmentId: number
): string {
  return `/projects/${projectId}/environments/${environmentId}`;
}

function containerBasePath(
  projectId: number,
  environmentId: number
): string {
  return `/projects/${projectId}/environments/${environmentId}/containers`;
}

export const tools: ToolDefinition[] = [
  {
    name: 'list_containers',
    description: 'List all containers in a project environment',
    inputSchema: {
      type: 'object',
      properties: {
        project_id: {
          type: 'number',
          description: 'Project ID',
        },
        environment_id: {
          type: 'number',
          description: 'Environment ID',
        },
      },
      required: ['project_id', 'environment_id'],
    },
    handler: (args) =>
      handleToolCall(async () => {
        const projectId = requireParam<number>(args, 'project_id');
        const environmentId = requireParam<number>(args, 'environment_id');
        const client = getClient();

        const result = await client.get<ContainerListResponse>(
          containerBasePath(projectId, environmentId)
        );

        if (!result.containers || result.containers.length === 0) {
          return ok('No containers found in this environment.');
        }

        const rows = result.containers.map((c) => [
          c.id,
          c.name,
          c.image,
          c.status,
        ]);

        return ok(
          `Total: ${result.total}\n\n` +
            table(['ID', 'Name', 'Image', 'Status'], rows)
        );
      }),
  },
  {
    name: 'get_container',
    description: 'Get details of a specific container',
    inputSchema: {
      type: 'object',
      properties: {
        project_id: {
          type: 'number',
          description: 'Project ID',
        },
        environment_id: {
          type: 'number',
          description: 'Environment ID',
        },
        container_id: {
          type: 'string',
          description: 'Container ID',
        },
      },
      required: ['project_id', 'environment_id', 'container_id'],
    },
    handler: (args) =>
      handleToolCall(async () => {
        const projectId = requireParam<number>(args, 'project_id');
        const environmentId = requireParam<number>(args, 'environment_id');
        const containerId = requireParam<string>(args, 'container_id');
        const client = getClient();

        const container = await client.get<ContainerDetail>(
          `${containerBasePath(projectId, environmentId)}/${containerId}`
        );

        return json('Container Details', container);
      }),
  },
  {
    name: 'start_container',
    description: 'Start a stopped container',
    inputSchema: {
      type: 'object',
      properties: {
        project_id: {
          type: 'number',
          description: 'Project ID',
        },
        environment_id: {
          type: 'number',
          description: 'Environment ID',
        },
        container_id: {
          type: 'string',
          description: 'Container ID',
        },
      },
      required: ['project_id', 'environment_id', 'container_id'],
    },
    handler: (args) =>
      handleToolCall(async () => {
        const projectId = requireParam<number>(args, 'project_id');
        const environmentId = requireParam<number>(args, 'environment_id');
        const containerId = requireParam<string>(args, 'container_id');
        const client = getClient();

        await client.post(
          `${containerBasePath(projectId, environmentId)}/${containerId}/start`
        );

        return ok(`Container ${containerId} started successfully.`);
      }),
  },
  {
    name: 'stop_container',
    description: 'Stop a running container',
    inputSchema: {
      type: 'object',
      properties: {
        project_id: {
          type: 'number',
          description: 'Project ID',
        },
        environment_id: {
          type: 'number',
          description: 'Environment ID',
        },
        container_id: {
          type: 'string',
          description: 'Container ID',
        },
      },
      required: ['project_id', 'environment_id', 'container_id'],
    },
    handler: (args) =>
      handleToolCall(async () => {
        const projectId = requireParam<number>(args, 'project_id');
        const environmentId = requireParam<number>(args, 'environment_id');
        const containerId = requireParam<string>(args, 'container_id');
        const client = getClient();

        await client.post(
          `${containerBasePath(projectId, environmentId)}/${containerId}/stop`
        );

        return ok(`Container ${containerId} stopped successfully.`);
      }),
  },
  {
    name: 'restart_container',
    description: 'Restart a container',
    inputSchema: {
      type: 'object',
      properties: {
        project_id: {
          type: 'number',
          description: 'Project ID',
        },
        environment_id: {
          type: 'number',
          description: 'Environment ID',
        },
        container_id: {
          type: 'string',
          description: 'Container ID',
        },
      },
      required: ['project_id', 'environment_id', 'container_id'],
    },
    handler: (args) =>
      handleToolCall(async () => {
        const projectId = requireParam<number>(args, 'project_id');
        const environmentId = requireParam<number>(args, 'environment_id');
        const containerId = requireParam<string>(args, 'container_id');
        const client = getClient();

        await client.post(
          `${containerBasePath(projectId, environmentId)}/${containerId}/restart`
        );

        return ok(`Container ${containerId} restarted successfully.`);
      }),
  },
  {
    name: 'get_container_metrics',
    description:
      'Get CPU, memory, and network metrics for a container',
    inputSchema: {
      type: 'object',
      properties: {
        project_id: {
          type: 'number',
          description: 'Project ID',
        },
        environment_id: {
          type: 'number',
          description: 'Environment ID',
        },
        container_id: {
          type: 'string',
          description: 'Container ID',
        },
      },
      required: ['project_id', 'environment_id', 'container_id'],
    },
    handler: (args) =>
      handleToolCall(async () => {
        const projectId = requireParam<number>(args, 'project_id');
        const environmentId = requireParam<number>(args, 'environment_id');
        const containerId = requireParam<string>(args, 'container_id');
        const client = getClient();

        const metrics = await client.get<ContainerMetrics>(
          `${containerBasePath(projectId, environmentId)}/${containerId}/metrics`
        );

        return json('Container Metrics', metrics);
      }),
  },
  {
    name: 'get_container_logs',
    description:
      'Get runtime logs for a container in an environment. Returns a snapshot of recent log output.',
    inputSchema: {
      type: 'object',
      properties: {
        project_id: {
          type: 'number',
          description: 'Project ID',
        },
        environment_id: {
          type: 'number',
          description: 'Environment ID',
        },
        container_id: {
          type: 'string',
          description:
            'Container ID. If omitted, logs from the primary container are returned.',
        },
        tail: {
          type: 'string',
          description:
            'Number of lines to return (default: "200"). Use "all" for full log history.',
        },
        timestamps: {
          type: 'boolean',
          description: 'Include timestamps in log output (default: false)',
        },
      },
      required: ['project_id', 'environment_id'],
    },
    handler: (args) =>
      handleToolCall(async () => {
        const projectId = requireParam<number>(args, 'project_id');
        const environmentId = requireParam<number>(args, 'environment_id');
        const containerId = optionalParam<string>(args, 'container_id');
        const tail = optionalParam<string>(args, 'tail') ?? '200';
        const timestamps = optionalParam<boolean>(args, 'timestamps') ?? false;
        const client = getClient();

        const query: Record<string, unknown> = {
          tail,
          timestamps: String(timestamps),
          follow: 'false',
        };

        // Use the container-specific endpoint if an ID is provided,
        // otherwise use the environment-level endpoint.
        const path = containerId
          ? `${containerBasePath(projectId, environmentId)}/${containerId}/logs`
          : `${envBasePath(projectId, environmentId)}/container-logs`;

        if (!containerId) {
          // The environment-level endpoint accepts an optional container_name filter
          // but we don't expose that here — it defaults to the primary container.
        }

        const lines = await client.ws(path, query);

        if (lines.length === 0) {
          return ok('No log output available for this container.');
        }

        return ok(
          `## Container Logs (last ${tail} lines)\n\n` +
            '```\n' +
            lines.join('\n') +
            '\n```'
        );
      }),
  },
];
