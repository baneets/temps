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

interface NotificationProvider {
  id: number;
  name: string;
  provider_type: string;
  enabled: boolean;
  config?: Record<string, unknown>;
  created_at?: string;
  updated_at?: string;
}

interface TestResult {
  success: boolean;
  message: string;
}

export const tools: ToolDefinition[] = [
  {
    name: 'list_notification_providers',
    description: 'List all notification providers',
    inputSchema: {
      type: 'object',
      properties: {},
    },
    handler: (args) =>
      handleToolCall(async () => {
        const client = getClient();
        const providers = await client.get<NotificationProvider[]>(
          '/notification-providers'
        );

        if (!providers || providers.length === 0) {
          return ok('No notification providers found.');
        }

        const rows = providers.map((p) => [
          String(p.id),
          p.name,
          p.provider_type,
          String(p.enabled),
        ]);

        return ok(
          table(['ID', 'Name', 'Type', 'Enabled'], rows)
        );
      }),
  },
  {
    name: 'create_notification_provider',
    description:
      'Create a new notification provider (slack, email, or webhook)',
    inputSchema: {
      type: 'object',
      properties: {
        name: {
          type: 'string',
          description: 'Provider name',
        },
        provider_type: {
          type: 'string',
          description: 'Provider type: slack, email, or webhook',
          enum: ['slack', 'email', 'webhook'],
        },
        config: {
          type: 'object',
          description:
            'Provider-specific configuration (e.g. webhook_url, email addresses)',
        },
        enabled: {
          type: 'boolean',
          description: 'Whether the provider is enabled (defaults to true)',
        },
      },
      required: ['name', 'provider_type', 'config'],
    },
    handler: (args) =>
      handleToolCall(async () => {
        const name = requireParam<string>(args, 'name');
        const providerType = requireParam<string>(args, 'provider_type');
        const config = requireParam<Record<string, unknown>>(args, 'config');
        const enabled = optionalParam<boolean>(args, 'enabled');
        const client = getClient();

        const body: Record<string, unknown> = {
          name,
          provider_type: providerType,
          config,
        };
        if (enabled !== undefined) body.enabled = enabled;

        const provider = await client.post<NotificationProvider>(
          '/notification-providers',
          body
        );

        return json('Notification Provider Created', provider);
      }),
  },
  {
    name: 'get_notification_provider',
    description: 'Get details of a specific notification provider',
    inputSchema: {
      type: 'object',
      properties: {
        id: {
          type: 'number',
          description: 'Notification provider ID',
        },
      },
      required: ['id'],
    },
    handler: (args) =>
      handleToolCall(async () => {
        const id = requireParam<number>(args, 'id');
        const client = getClient();
        const provider = await client.get<NotificationProvider>(
          `/notification-providers/${id}`
        );

        return json('Notification Provider Details', provider);
      }),
  },
  {
    name: 'update_notification_provider',
    description: 'Update a notification provider',
    inputSchema: {
      type: 'object',
      properties: {
        id: {
          type: 'number',
          description: 'Notification provider ID',
        },
        name: {
          type: 'string',
          description: 'New provider name',
        },
        enabled: {
          type: 'boolean',
          description: 'Whether the provider is enabled',
        },
      },
      required: ['id'],
    },
    handler: (args) =>
      handleToolCall(async () => {
        const id = requireParam<number>(args, 'id');
        const name = optionalParam<string>(args, 'name');
        const enabled = optionalParam<boolean>(args, 'enabled');
        const client = getClient();

        const body: Record<string, unknown> = {};
        if (name !== undefined) body.name = name;
        if (enabled !== undefined) body.enabled = enabled;

        const provider = await client.put<NotificationProvider>(
          `/notification-providers/${id}`,
          body
        );

        return json('Notification Provider Updated', provider);
      }),
  },
  {
    name: 'delete_notification_provider',
    description: 'Delete a notification provider',
    inputSchema: {
      type: 'object',
      properties: {
        id: {
          type: 'number',
          description: 'Notification provider ID',
        },
      },
      required: ['id'],
    },
    handler: (args) =>
      handleToolCall(async () => {
        const id = requireParam<number>(args, 'id');
        const client = getClient();
        await client.delete(`/notification-providers/${id}`);

        return ok(`Notification provider ${id} deleted successfully.`);
      }),
  },
  {
    name: 'enable_notification_provider',
    description: 'Enable a notification provider',
    inputSchema: {
      type: 'object',
      properties: {
        id: {
          type: 'number',
          description: 'Notification provider ID',
        },
      },
      required: ['id'],
    },
    handler: (args) =>
      handleToolCall(async () => {
        const id = requireParam<number>(args, 'id');
        const client = getClient();
        await client.put(`/notification-providers/${id}`, {
          enabled: true,
        });

        return ok(`Notification provider ${id} enabled successfully.`);
      }),
  },
  {
    name: 'disable_notification_provider',
    description: 'Disable a notification provider',
    inputSchema: {
      type: 'object',
      properties: {
        id: {
          type: 'number',
          description: 'Notification provider ID',
        },
      },
      required: ['id'],
    },
    handler: (args) =>
      handleToolCall(async () => {
        const id = requireParam<number>(args, 'id');
        const client = getClient();
        await client.put(`/notification-providers/${id}`, {
          enabled: false,
        });

        return ok(`Notification provider ${id} disabled successfully.`);
      }),
  },
  {
    name: 'test_notification_provider',
    description:
      'Send a test notification through a provider to verify it works',
    inputSchema: {
      type: 'object',
      properties: {
        id: {
          type: 'number',
          description: 'Notification provider ID',
        },
      },
      required: ['id'],
    },
    handler: (args) =>
      handleToolCall(async () => {
        const id = requireParam<number>(args, 'id');
        const client = getClient();
        const result = await client.post<TestResult>(
          `/notification-providers/${id}/test`
        );

        if (result.success) {
          return ok(`Test notification sent successfully: ${result.message}`);
        }

        return ok(`Test notification failed: ${result.message}`);
      }),
  },
];
