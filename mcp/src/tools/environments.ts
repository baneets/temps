/**
 * Environment and environment variable management tools
 */

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

export const tools: ToolDefinition[] = [
  {
    name: 'list_environments',
    description: 'List all environments for a project',
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
        const envs = await client.get<any[]>(
          `/projects/${projectId}/environments`
        );
        const rows = envs.map((e: any) => [
          String(e.id),
          e.name ?? '',
          e.branch ?? '',
          e.is_preview ? 'Yes' : 'No',
          e.status ?? '',
          formatDate(e.created_at),
        ]);
        return ok(
          `## Environments for Project ${projectId}\n\n${table(
            ['ID', 'Name', 'Branch', 'Preview', 'Status', 'Created'],
            rows
          )}`
        );
      }),
  },

  {
    name: 'create_environment',
    description: 'Create a new environment for a project',
    inputSchema: {
      type: 'object',
      properties: {
        project_id: {
          type: 'number',
          description: 'The project ID',
        },
        name: {
          type: 'string',
          description: 'Environment name',
        },
        branch: {
          type: 'string',
          description: 'Git branch to deploy from',
        },
        set_as_preview: {
          type: 'boolean',
          description: 'Whether to set this as the preview environment',
        },
      },
      required: ['project_id', 'name', 'branch'],
    },
    handler: (args) =>
      handleToolCall(async () => {
        const client = getClient();
        const projectId = requireParam<number>(args, 'project_id');
        const name = requireParam<string>(args, 'name');
        const branch = requireParam<string>(args, 'branch');
        const setAsPreview = optionalParam<boolean>(args, 'set_as_preview');

        const body: Record<string, unknown> = { name, branch };
        if (setAsPreview !== undefined) body.set_as_preview = setAsPreview;

        const env = await client.post<any>(
          `/projects/${projectId}/environments`,
          body
        );
        return json('Environment Created', env);
      }),
  },

  {
    name: 'delete_environment',
    description: 'Delete an environment from a project',
    inputSchema: {
      type: 'object',
      properties: {
        project_id: {
          type: 'number',
          description: 'The project ID',
        },
        environment_id: {
          type: 'number',
          description: 'The environment ID',
        },
      },
      required: ['project_id', 'environment_id'],
    },
    handler: (args) =>
      handleToolCall(async () => {
        const client = getClient();
        const projectId = requireParam<number>(args, 'project_id');
        const envId = requireParam<number>(args, 'environment_id');
        await client.delete(`/projects/${projectId}/environments/${envId}`);
        return ok(
          `Environment ${envId} deleted from project ${projectId}.`
        );
      }),
  },

  {
    name: 'list_environment_variables',
    description: 'List all environment variables for a project',
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
        const vars = await client.get<any[]>(
          `/projects/${projectId}/env-vars`
        );
        const rows = vars.map((v: any) => [
          String(v.id),
          v.key ?? '',
          v.value ?? '***',
          Array.isArray(v.environment_ids) ? v.environment_ids.join(', ') : '',
        ]);
        return ok(
          `## Environment Variables for Project ${projectId}\n\n${table(
            ['ID', 'Key', 'Value', 'Environment IDs'],
            rows
          )}`
        );
      }),
  },

  {
    name: 'get_environment_variable',
    description: 'Get the value of a specific environment variable',
    inputSchema: {
      type: 'object',
      properties: {
        project_id: {
          type: 'number',
          description: 'The project ID',
        },
        variable_id: {
          type: 'number',
          description: 'The environment variable ID',
        },
      },
      required: ['project_id', 'variable_id'],
    },
    handler: (args) =>
      handleToolCall(async () => {
        const client = getClient();
        const projectId = requireParam<number>(args, 'project_id');
        const varId = requireParam<number>(args, 'variable_id');
        const result = await client.get<any>(
          `/projects/${projectId}/env-vars/${varId}/value`
        );
        return json('Environment Variable Value', result);
      }),
  },

  {
    name: 'set_environment_variable',
    description: 'Create a new environment variable for a project',
    inputSchema: {
      type: 'object',
      properties: {
        project_id: {
          type: 'number',
          description: 'The project ID',
        },
        key: {
          type: 'string',
          description: 'Variable name',
        },
        value: {
          type: 'string',
          description: 'Variable value',
        },
        environment_ids: {
          type: 'array',
          items: { type: 'number' },
          description:
            'List of environment IDs to attach this variable to. If omitted, applies to all environments.',
        },
        include_in_preview: {
          type: 'boolean',
          description: 'Whether to include this variable in preview environments',
        },
      },
      required: ['project_id', 'key', 'value'],
    },
    handler: (args) =>
      handleToolCall(async () => {
        const client = getClient();
        const projectId = requireParam<number>(args, 'project_id');
        const key = requireParam<string>(args, 'key');
        const value = requireParam<string>(args, 'value');
        const environmentIds = optionalParam<number[]>(args, 'environment_ids');
        const includeInPreview = optionalParam<boolean>(
          args,
          'include_in_preview'
        );

        const body: Record<string, unknown> = { key, value, environment_ids: environmentIds ?? [] };
        if (includeInPreview !== undefined)
          body.include_in_preview = includeInPreview;

        const result = await client.post<any>(
          `/projects/${projectId}/env-vars`,
          body
        );
        return json('Environment Variable Created', result);
      }),
  },

  {
    name: 'update_environment_variable',
    description: 'Update an existing environment variable',
    inputSchema: {
      type: 'object',
      properties: {
        project_id: {
          type: 'number',
          description: 'The project ID',
        },
        variable_id: {
          type: 'number',
          description: 'The environment variable ID',
        },
        key: {
          type: 'string',
          description: 'New variable name',
        },
        value: {
          type: 'string',
          description: 'New variable value',
        },
        environment_ids: {
          type: 'array',
          items: { type: 'number' },
          description: 'Updated list of environment IDs',
        },
        include_in_preview: {
          type: 'boolean',
          description: 'Whether to include this variable in preview environments',
        },
      },
      required: ['project_id', 'variable_id'],
    },
    handler: (args) =>
      handleToolCall(async () => {
        const client = getClient();
        const projectId = requireParam<number>(args, 'project_id');
        const varId = requireParam<number>(args, 'variable_id');
        const key = optionalParam<string>(args, 'key');
        const value = optionalParam<string>(args, 'value');
        const environmentIds = optionalParam<number[]>(args, 'environment_ids');
        const includeInPreview = optionalParam<boolean>(
          args,
          'include_in_preview'
        );

        // API requires key, value, and environment_ids; fetch current to merge
        const allVars = await client.get<any[]>(`/projects/${projectId}/env-vars`);
        const currentVar = allVars.find((v: any) => v.id === varId);

        const body: Record<string, unknown> = {
          key: key ?? currentVar?.key ?? '',
          value: value ?? currentVar?.value ?? '',
          environment_ids: environmentIds ?? currentVar?.environment_ids ?? [],
        };
        if (includeInPreview !== undefined)
          body.include_in_preview = includeInPreview;

        const result = await client.put<any>(
          `/projects/${projectId}/env-vars/${varId}`,
          body
        );
        return json('Environment Variable Updated', result);
      }),
  },

  {
    name: 'delete_environment_variable',
    description: 'Delete an environment variable from a project',
    inputSchema: {
      type: 'object',
      properties: {
        project_id: {
          type: 'number',
          description: 'The project ID',
        },
        variable_id: {
          type: 'number',
          description: 'The environment variable ID',
        },
      },
      required: ['project_id', 'variable_id'],
    },
    handler: (args) =>
      handleToolCall(async () => {
        const client = getClient();
        const projectId = requireParam<number>(args, 'project_id');
        const varId = requireParam<number>(args, 'variable_id');
        await client.delete(
          `/projects/${projectId}/env-vars/${varId}`
        );
        return ok(
          `Environment variable ${varId} deleted from project ${projectId}.`
        );
      }),
  },

  {
    name: 'update_environment_resources',
    description:
      'Update CPU and memory resource limits for an environment',
    inputSchema: {
      type: 'object',
      properties: {
        project_id: {
          type: 'number',
          description: 'The project ID',
        },
        environment_id: {
          type: 'number',
          description: 'The environment ID',
        },
        cpu_limit: {
          type: 'string',
          description: 'CPU limit (e.g. "500m", "1")',
        },
        memory_limit: {
          type: 'string',
          description: 'Memory limit (e.g. "256Mi", "1Gi")',
        },
        cpu_request: {
          type: 'string',
          description: 'CPU request (e.g. "100m", "0.5")',
        },
        memory_request: {
          type: 'string',
          description: 'Memory request (e.g. "128Mi", "512Mi")',
        },
      },
      required: ['project_id', 'environment_id'],
    },
    handler: (args) =>
      handleToolCall(async () => {
        const client = getClient();
        const projectId = requireParam<number>(args, 'project_id');
        const envId = requireParam<number>(args, 'environment_id');
        const cpuLimit = optionalParam<string>(args, 'cpu_limit');
        const memoryLimit = optionalParam<string>(args, 'memory_limit');
        const cpuRequest = optionalParam<string>(args, 'cpu_request');
        const memoryRequest = optionalParam<string>(args, 'memory_request');

        const body: Record<string, unknown> = {};
        if (cpuLimit !== undefined) body.cpu_limit = cpuLimit;
        if (memoryLimit !== undefined) body.memory_limit = memoryLimit;
        if (cpuRequest !== undefined) body.cpu_request = cpuRequest;
        if (memoryRequest !== undefined) body.memory_request = memoryRequest;

        const result = await client.patch<any>(
          `/projects/${projectId}/environments/${envId}/settings`,
          body
        );
        return json('Environment Resources Updated', result);
      }),
  },

  {
    name: 'scale_environment',
    description: 'Scale the number of replicas for an environment',
    inputSchema: {
      type: 'object',
      properties: {
        project_id: {
          type: 'number',
          description: 'The project ID',
        },
        environment_id: {
          type: 'number',
          description: 'The environment ID',
        },
        replicas: {
          type: 'number',
          description: 'Number of replicas to scale to',
        },
      },
      required: ['project_id', 'environment_id', 'replicas'],
    },
    handler: (args) =>
      handleToolCall(async () => {
        const client = getClient();
        const projectId = requireParam<number>(args, 'project_id');
        const envId = requireParam<number>(args, 'environment_id');
        const replicas = requireParam<number>(args, 'replicas');

        const result = await client.patch<any>(
          `/projects/${projectId}/environments/${envId}/settings`,
          { replicas }
        );
        return json('Environment Scaled', result);
      }),
  },

  {
    name: 'list_environment_crons',
    description: 'List cron jobs for an environment',
    inputSchema: {
      type: 'object',
      properties: {
        project_id: {
          type: 'number',
          description: 'The project ID',
        },
        environment_id: {
          type: 'number',
          description: 'The environment ID',
        },
      },
      required: ['project_id', 'environment_id'],
    },
    handler: (args) =>
      handleToolCall(async () => {
        const client = getClient();
        const projectId = requireParam<number>(args, 'project_id');
        const envId = requireParam<number>(args, 'environment_id');
        const crons = await client.get<any[]>(
          `/projects/${projectId}/environments/${envId}/crons`
        );
        return json('Environment Crons', crons);
      }),
  },

  {
    name: 'get_cron_executions',
    description: 'Get execution history for a specific cron job',
    inputSchema: {
      type: 'object',
      properties: {
        project_id: {
          type: 'number',
          description: 'The project ID',
        },
        environment_id: {
          type: 'number',
          description: 'The environment ID',
        },
        cron_id: {
          type: 'number',
          description: 'The cron job ID',
        },
        page: {
          type: 'number',
          description: 'Page number (default: 1)',
        },
        per_page: {
          type: 'number',
          description: 'Items per page (default: 20, max: 100)',
        },
      },
      required: ['project_id', 'environment_id', 'cron_id'],
    },
    handler: (args) =>
      handleToolCall(async () => {
        const client = getClient();
        const projectId = requireParam<number>(args, 'project_id');
        const envId = requireParam<number>(args, 'environment_id');
        const cronId = requireParam<number>(args, 'cron_id');
        const page = optionalParam<number>(args, 'page');
        const perPage = optionalParam<number>(args, 'per_page');

        const query: Record<string, unknown> = {};
        if (page !== undefined) query.page = page;
        if (perPage !== undefined) query.per_page = perPage;

        const executions = await client.get<any[]>(
          `/projects/${projectId}/environments/${envId}/crons/${cronId}/executions`,
          query
        );
        return json('Cron Executions', executions);
      }),
  },

  {
    name: 'teardown_environment',
    description:
      'Teardown an environment, removing all deployed resources while keeping the configuration',
    inputSchema: {
      type: 'object',
      properties: {
        project_id: {
          type: 'number',
          description: 'The project ID',
        },
        environment_id: {
          type: 'number',
          description: 'The environment ID',
        },
      },
      required: ['project_id', 'environment_id'],
    },
    handler: (args) =>
      handleToolCall(async () => {
        const client = getClient();
        const projectId = requireParam<number>(args, 'project_id');
        const envId = requireParam<number>(args, 'environment_id');
        await client.post(
          `/projects/${projectId}/environments/${envId}/teardown`
        );
        return ok(
          `Environment ${envId} teardown initiated for project ${projectId}.`
        );
      }),
  },
];
