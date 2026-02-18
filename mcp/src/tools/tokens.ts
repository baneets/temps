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

interface DeploymentToken {
  id: number;
  name: string;
  permissions?: string[];
  expires_at?: string;
  created_at?: string;
  last_used_at?: string;
  token?: string;
  [key: string]: unknown;
}

export const tools: ToolDefinition[] = [
  {
    name: 'list_deployment_tokens',
    description: 'List all deployment tokens for a project',
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
        const project_id = requireParam<number>(args, 'project_id');
        const data = await client.get<{
          tokens: DeploymentToken[];
          total: number;
        }>(`/projects/${project_id}/deployment-tokens`);
        const tokens = data.tokens ?? [];

        if (tokens.length === 0) {
          return ok(`No deployment tokens found for project ${project_id}.`);
        }

        const rows = tokens.map((t) => [
          String(t.id),
          t.name,
          formatDate(t.expires_at),
          formatDate(t.last_used_at),
          formatDate(t.created_at),
        ]);

        return ok(
          `## Deployment Tokens (${data.total})\n\n${table(['ID', 'Name', 'Expires', 'Last Used', 'Created'], rows)}`
        );
      }),
  },

  {
    name: 'create_deployment_token',
    description:
      'Create a new deployment token for a project. The token value is only shown once in the response.',
    inputSchema: {
      type: 'object',
      properties: {
        project_id: {
          type: 'number',
          description: 'Project ID',
        },
        name: {
          type: 'string',
          description: 'Name for the deployment token',
        },
        permissions: {
          type: 'array',
          items: { type: 'string' },
          description: 'Optional list of permissions for the token',
        },
        expires_at: {
          type: 'string',
          description: 'Optional expiration date (ISO 8601)',
        },
      },
      required: ['project_id', 'name'],
    },
    handler: (args) =>
      handleToolCall(async () => {
        const client = getClient();
        const project_id = requireParam<number>(args, 'project_id');
        const name = requireParam<string>(args, 'name');
        const permissions = optionalParam<string[]>(args, 'permissions');
        const expires_at = optionalParam<string>(args, 'expires_at');

        const body: Record<string, unknown> = { name };
        if (permissions) body.permissions = permissions;
        if (expires_at) body.expires_at = expires_at;

        const result = await client.post<DeploymentToken>(
          `/projects/${project_id}/deployment-tokens`,
          body
        );

        let text = `## Deployment Token Created\n\n`;
        if (result.token) {
          text += `**Token (save this -- it won't be shown again):**\n\`\`\`\n${result.token}\n\`\`\`\n\n`;
        }
        text += `\`\`\`json\n${JSON.stringify(result, null, 2)}\n\`\`\``;

        return ok(text);
      }),
  },

  {
    name: 'get_deployment_token',
    description: 'Get details of a specific deployment token',
    inputSchema: {
      type: 'object',
      properties: {
        project_id: {
          type: 'number',
          description: 'Project ID',
        },
        token_id: {
          type: 'number',
          description: 'Deployment token ID',
        },
      },
      required: ['project_id', 'token_id'],
    },
    handler: (args) =>
      handleToolCall(async () => {
        const client = getClient();
        const project_id = requireParam<number>(args, 'project_id');
        const token_id = requireParam<number>(args, 'token_id');
        const token = await client.get<DeploymentToken>(
          `/projects/${project_id}/deployment-tokens/${token_id}`
        );
        return json('Deployment Token Details', token);
      }),
  },

  {
    name: 'delete_deployment_token',
    description: 'Delete a deployment token from a project',
    inputSchema: {
      type: 'object',
      properties: {
        project_id: {
          type: 'number',
          description: 'Project ID',
        },
        token_id: {
          type: 'number',
          description: 'Deployment token ID',
        },
      },
      required: ['project_id', 'token_id'],
    },
    handler: (args) =>
      handleToolCall(async () => {
        const client = getClient();
        const project_id = requireParam<number>(args, 'project_id');
        const token_id = requireParam<number>(args, 'token_id');
        await client.delete(
          `/projects/${project_id}/deployment-tokens/${token_id}`
        );
        return ok(
          `Deployment token ${token_id} deleted from project ${project_id} successfully.`
        );
      }),
  },
];
