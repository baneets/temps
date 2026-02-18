import { getClient } from '../api/index.js';
import { ok, json, table, formatDate, handleToolCall, requireParam, optionalParam } from './_helpers.js';
import type { ToolDefinition } from '../types/index.js';

export const tools: ToolDefinition[] = [
  {
    name: 'list_api_keys',
    description: 'List all API keys for the current account',
    inputSchema: {
      type: 'object',
      properties: {},
    },
    handler: (args) =>
      handleToolCall(async () => {
        const client = getClient();
        const data = await client.get<{ api_keys: Array<Record<string, unknown>> }>('/api-keys');
        const keys = data.api_keys ?? [];

        if (keys.length === 0) {
          return ok('No API keys found.');
        }

        const rows = keys.map((k) => [
          String(k.id ?? ''),
          String(k.name ?? ''),
          String(k.role_type ?? ''),
          String(k.is_active ?? ''),
          formatDate(k.last_used_at as string | null),
        ]);

        return ok(
          `## API Keys (${keys.length})\n\n${table(['ID', 'Name', 'Role', 'Active', 'Last Used'], rows)}`
        );
      }),
  },
  {
    name: 'create_api_key',
    description: 'Create a new API key. The key value is only shown once in the response.',
    inputSchema: {
      type: 'object',
      properties: {
        name: { type: 'string', description: 'Name for the API key' },
        role_type: { type: 'string', description: 'Role type for the key (e.g. admin, viewer)' },
        expires_at: { type: 'string', description: 'Optional expiration date (ISO 8601)' },
        permissions: {
          type: 'array',
          items: { type: 'string' },
          description: 'Optional list of specific permissions',
        },
      },
      required: ['name', 'role_type'],
    },
    handler: (args) =>
      handleToolCall(async () => {
        const client = getClient();
        const name = requireParam<string>(args, 'name');
        const role_type = requireParam<string>(args, 'role_type');
        const expires_at = optionalParam<string>(args, 'expires_at');
        const permissions = optionalParam<string[]>(args, 'permissions');

        const body: Record<string, unknown> = { name, role_type };
        if (expires_at) body.expires_at = expires_at;
        if (permissions) body.permissions = permissions;

        const result = await client.post<Record<string, unknown>>('/api-keys', body);
        const apiKey = result.api_key as string | undefined;

        let text = `## API Key Created\n\n`;
        if (apiKey) {
          text += `**API Key (save this — it won't be shown again):**\n\`\`\`\n${apiKey}\n\`\`\`\n\n`;
        }
        text += `\`\`\`json\n${JSON.stringify(result, null, 2)}\n\`\`\``;

        return ok(text);
      }),
  },
  {
    name: 'get_api_key',
    description: 'Get details of a specific API key by ID',
    inputSchema: {
      type: 'object',
      properties: {
        id: { type: 'number', description: 'API key ID' },
      },
      required: ['id'],
    },
    handler: (args) =>
      handleToolCall(async () => {
        const client = getClient();
        const id = requireParam<number>(args, 'id');
        const data = await client.get<Record<string, unknown>>(`/api-keys/${id}`);
        return json('API Key Details', data);
      }),
  },
  {
    name: 'delete_api_key',
    description: 'Delete an API key by ID',
    inputSchema: {
      type: 'object',
      properties: {
        id: { type: 'number', description: 'API key ID' },
      },
      required: ['id'],
    },
    handler: (args) =>
      handleToolCall(async () => {
        const client = getClient();
        const id = requireParam<number>(args, 'id');
        await client.delete(`/api-keys/${id}`);
        return ok(`API key ${id} deleted successfully.`);
      }),
  },
  {
    name: 'activate_api_key',
    description: 'Activate a deactivated API key',
    inputSchema: {
      type: 'object',
      properties: {
        id: { type: 'number', description: 'API key ID' },
      },
      required: ['id'],
    },
    handler: (args) =>
      handleToolCall(async () => {
        const client = getClient();
        const id = requireParam<number>(args, 'id');
        await client.post(`/api-keys/${id}/activate`);
        return ok(`API key ${id} activated successfully.`);
      }),
  },
  {
    name: 'deactivate_api_key',
    description: 'Deactivate an active API key',
    inputSchema: {
      type: 'object',
      properties: {
        id: { type: 'number', description: 'API key ID' },
      },
      required: ['id'],
    },
    handler: (args) =>
      handleToolCall(async () => {
        const client = getClient();
        const id = requireParam<number>(args, 'id');
        await client.post(`/api-keys/${id}/deactivate`);
        return ok(`API key ${id} deactivated successfully.`);
      }),
  },
  {
    name: 'get_api_key_permissions',
    description: 'List all available API key permissions grouped by category',
    inputSchema: {
      type: 'object',
      properties: {},
    },
    handler: (args) =>
      handleToolCall(async () => {
        const client = getClient();
        const data = await client.get<{ permissions: unknown }>('/api-keys/permissions');
        return json('API Key Permissions', data.permissions ?? data);
      }),
  },
];
