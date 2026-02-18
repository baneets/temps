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

interface Dsn {
  id: number;
  name?: string;
  dsn: string;
  project_id: number;
  environment_id?: number;
  deployment_id?: number;
  created_at?: string;
}

export const tools: ToolDefinition[] = [
  {
    name: 'list_dsns',
    description: 'List all DSNs (Data Source Names) for a project',
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
        const client = getClient();
        const projectId = requireParam<number>(args, 'project_id');
        const dsns = await client.get<Dsn[]>(
          `/projects/${projectId}/dsns`
        );

        if (!dsns || dsns.length === 0) {
          return ok('No DSNs found for this project.');
        }

        const rows = dsns.map((d) => [
          String(d.id),
          d.name ?? '',
          d.dsn ?? '',
        ]);

        return ok(
          `## DSNs for Project ${projectId}\n\n${table(['ID', 'Name', 'DSN'], rows)}`
        );
      }),
  },

  {
    name: 'create_dsn',
    description: 'Create a new DSN for a project',
    inputSchema: {
      type: 'object',
      properties: {
        project_id: {
          type: 'number',
          description: 'Project ID',
        },
        name: {
          type: 'string',
          description: 'Optional name for the DSN',
        },
        environment_id: {
          type: 'number',
          description: 'Optional environment ID to scope the DSN to',
        },
        deployment_id: {
          type: 'number',
          description: 'Optional deployment ID to scope the DSN to',
        },
        base_url: {
          type: 'string',
          description: 'Optional base URL for the DSN',
        },
      },
      required: ['project_id'],
    },
    handler: (args) =>
      handleToolCall(async () => {
        const client = getClient();
        const projectId = requireParam<number>(args, 'project_id');
        const name = optionalParam<string>(args, 'name');
        const environmentId = optionalParam<number>(args, 'environment_id');
        const deploymentId = optionalParam<number>(args, 'deployment_id');
        const baseUrl = optionalParam<string>(args, 'base_url');

        const body: Record<string, unknown> = {};
        if (name !== undefined) body.name = name;
        if (environmentId !== undefined) body.environment_id = environmentId;
        if (deploymentId !== undefined) body.deployment_id = deploymentId;
        if (baseUrl !== undefined) body.base_url = baseUrl;

        const dsn = await client.post<Dsn>(
          `/projects/${projectId}/dsns`,
          body
        );
        return json('DSN Created', dsn);
      }),
  },

  {
    name: 'get_or_create_dsn',
    description:
      'Get an existing DSN matching the criteria or create a new one for a project',
    inputSchema: {
      type: 'object',
      properties: {
        project_id: {
          type: 'number',
          description: 'Project ID',
        },
        environment_id: {
          type: 'number',
          description: 'Optional environment ID',
        },
        deployment_id: {
          type: 'number',
          description: 'Optional deployment ID',
        },
        base_url: {
          type: 'string',
          description: 'Optional base URL for the DSN',
        },
      },
      required: ['project_id'],
    },
    handler: (args) =>
      handleToolCall(async () => {
        const client = getClient();
        const projectId = requireParam<number>(args, 'project_id');
        const environmentId = optionalParam<number>(args, 'environment_id');
        const deploymentId = optionalParam<number>(args, 'deployment_id');
        const baseUrl = optionalParam<string>(args, 'base_url');

        const body: Record<string, unknown> = {};
        if (environmentId !== undefined) body.environment_id = environmentId;
        if (deploymentId !== undefined) body.deployment_id = deploymentId;
        if (baseUrl !== undefined) body.base_url = baseUrl;

        const dsn = await client.post<Dsn>(
          `/projects/${projectId}/dsns/get-or-create`,
          body
        );
        return json('DSN', dsn);
      }),
  },

  {
    name: 'regenerate_dsn',
    description: 'Regenerate the key for an existing DSN',
    inputSchema: {
      type: 'object',
      properties: {
        project_id: {
          type: 'number',
          description: 'Project ID',
        },
        dsn_id: {
          type: 'number',
          description: 'DSN ID to regenerate',
        },
        base_url: {
          type: 'string',
          description: 'Optional new base URL for the regenerated DSN',
        },
      },
      required: ['project_id', 'dsn_id'],
    },
    handler: (args) =>
      handleToolCall(async () => {
        const client = getClient();
        const projectId = requireParam<number>(args, 'project_id');
        const dsnId = requireParam<number>(args, 'dsn_id');
        const baseUrl = optionalParam<string>(args, 'base_url');

        const body: Record<string, unknown> = {};
        if (baseUrl !== undefined) body.base_url = baseUrl;

        const dsn = await client.post<Dsn>(
          `/projects/${projectId}/dsns/${dsnId}/regenerate`,
          body
        );
        return json('DSN Regenerated', dsn);
      }),
  },

  {
    name: 'revoke_dsn',
    description: 'Revoke (delete) a DSN from a project',
    inputSchema: {
      type: 'object',
      properties: {
        project_id: {
          type: 'number',
          description: 'Project ID',
        },
        dsn_id: {
          type: 'number',
          description: 'DSN ID to revoke',
        },
      },
      required: ['project_id', 'dsn_id'],
    },
    handler: (args) =>
      handleToolCall(async () => {
        const client = getClient();
        const projectId = requireParam<number>(args, 'project_id');
        const dsnId = requireParam<number>(args, 'dsn_id');
        await client.post(`/projects/${projectId}/dsns/${dsnId}/revoke`);
        return ok(`DSN ${dsnId} revoked from project ${projectId}.`);
      }),
  },
];
