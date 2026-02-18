import { getClient } from '../api/index.js';
import { ok, json, table, formatDate, handleToolCall, requireParam, optionalParam } from './_helpers.js';
import type { ToolDefinition } from '../types/index.js';

export const tools: ToolDefinition[] = [
  {
    name: 'list_webhooks',
    description: 'List all webhooks for a project',
    inputSchema: {
      type: 'object',
      properties: {
        project_id: { type: 'number', description: 'Project ID' },
      },
      required: ['project_id'],
    },
    handler: (args) =>
      handleToolCall(async () => {
        const client = getClient();
        const projectId = requireParam<number>(args, 'project_id');
        const data = await client.get<Array<Record<string, unknown>>>(
          `/projects/${projectId}/webhooks`
        );
        const webhooks = Array.isArray(data) ? data : [];

        if (webhooks.length === 0) {
          return ok('No webhooks found for this project.');
        }

        const rows = webhooks.map((w) => [
          String(w.id ?? ''),
          String(w.url ?? ''),
          Array.isArray(w.events) ? w.events.join(', ') : String(w.events ?? ''),
          String(w.enabled ?? ''),
        ]);

        return ok(
          `## Webhooks (${webhooks.length})\n\n${table(['ID', 'URL', 'Events', 'Enabled'], rows)}`
        );
      }),
  },
  {
    name: 'create_webhook',
    description: 'Create a new webhook for a project',
    inputSchema: {
      type: 'object',
      properties: {
        project_id: { type: 'number', description: 'Project ID' },
        url: { type: 'string', description: 'Webhook endpoint URL' },
        events: {
          type: 'array',
          items: { type: 'string' },
          description: 'List of event types to subscribe to',
        },
        secret: { type: 'string', description: 'Optional secret for webhook signature verification' },
        enabled: { type: 'boolean', description: 'Whether the webhook is enabled (default: true)' },
      },
      required: ['project_id', 'url', 'events'],
    },
    handler: (args) =>
      handleToolCall(async () => {
        const client = getClient();
        const projectId = requireParam<number>(args, 'project_id');
        const url = requireParam<string>(args, 'url');
        const events = requireParam<string[]>(args, 'events');
        const secret = optionalParam<string>(args, 'secret');
        const enabled = optionalParam<boolean>(args, 'enabled');

        const body: Record<string, unknown> = { url, events };
        if (secret !== undefined) body.secret = secret;
        if (enabled !== undefined) body.enabled = enabled;

        const result = await client.post<Record<string, unknown>>(
          `/projects/${projectId}/webhooks`,
          body
        );
        return json('Webhook Created', result);
      }),
  },
  {
    name: 'get_webhook',
    description: 'Get details of a specific webhook',
    inputSchema: {
      type: 'object',
      properties: {
        project_id: { type: 'number', description: 'Project ID' },
        webhook_id: { type: 'number', description: 'Webhook ID' },
      },
      required: ['project_id', 'webhook_id'],
    },
    handler: (args) =>
      handleToolCall(async () => {
        const client = getClient();
        const projectId = requireParam<number>(args, 'project_id');
        const webhookId = requireParam<number>(args, 'webhook_id');
        const data = await client.get<Record<string, unknown>>(
          `/projects/${projectId}/webhooks/${webhookId}`
        );
        return json('Webhook Details', data);
      }),
  },
  {
    name: 'update_webhook',
    description: 'Update an existing webhook',
    inputSchema: {
      type: 'object',
      properties: {
        project_id: { type: 'number', description: 'Project ID' },
        webhook_id: { type: 'number', description: 'Webhook ID' },
        url: { type: 'string', description: 'Updated webhook endpoint URL' },
        events: {
          type: 'array',
          items: { type: 'string' },
          description: 'Updated list of event types',
        },
        secret: { type: 'string', description: 'Updated secret for signature verification' },
        enabled: { type: 'boolean', description: 'Whether the webhook is enabled' },
      },
      required: ['project_id', 'webhook_id'],
    },
    handler: (args) =>
      handleToolCall(async () => {
        const client = getClient();
        const projectId = requireParam<number>(args, 'project_id');
        const webhookId = requireParam<number>(args, 'webhook_id');
        const url = optionalParam<string>(args, 'url');
        const events = optionalParam<string[]>(args, 'events');
        const secret = optionalParam<string>(args, 'secret');
        const enabled = optionalParam<boolean>(args, 'enabled');

        const body: Record<string, unknown> = {};
        if (url !== undefined) body.url = url;
        if (events !== undefined) body.events = events;
        if (secret !== undefined) body.secret = secret;
        if (enabled !== undefined) body.enabled = enabled;

        const result = await client.put<Record<string, unknown>>(
          `/projects/${projectId}/webhooks/${webhookId}`,
          body
        );
        return json('Webhook Updated', result);
      }),
  },
  {
    name: 'delete_webhook',
    description: 'Delete a webhook',
    inputSchema: {
      type: 'object',
      properties: {
        project_id: { type: 'number', description: 'Project ID' },
        webhook_id: { type: 'number', description: 'Webhook ID' },
      },
      required: ['project_id', 'webhook_id'],
    },
    handler: (args) =>
      handleToolCall(async () => {
        const client = getClient();
        const projectId = requireParam<number>(args, 'project_id');
        const webhookId = requireParam<number>(args, 'webhook_id');
        await client.delete(`/projects/${projectId}/webhooks/${webhookId}`);
        return ok(`Webhook ${webhookId} deleted successfully.`);
      }),
  },
  {
    name: 'list_webhook_event_types',
    description: 'List all available webhook event types grouped by category',
    inputSchema: {
      type: 'object',
      properties: {},
    },
    handler: (args) =>
      handleToolCall(async () => {
        const client = getClient();
        const data = await client.get<unknown>('/webhook-event-types');
        return json('Webhook Event Types', data);
      }),
  },
  {
    name: 'list_webhook_deliveries',
    description: 'List recent deliveries for a webhook',
    inputSchema: {
      type: 'object',
      properties: {
        project_id: { type: 'number', description: 'Project ID' },
        webhook_id: { type: 'number', description: 'Webhook ID' },
        limit: { type: 'number', description: 'Maximum number of deliveries to return' },
      },
      required: ['project_id', 'webhook_id'],
    },
    handler: (args) =>
      handleToolCall(async () => {
        const client = getClient();
        const projectId = requireParam<number>(args, 'project_id');
        const webhookId = requireParam<number>(args, 'webhook_id');
        const limit = optionalParam<number>(args, 'limit');

        const query: Record<string, unknown> = {};
        if (limit !== undefined) query.limit = limit;

        const data = await client.get<unknown>(
          `/projects/${projectId}/webhooks/${webhookId}/deliveries`,
          query
        );
        return json('Webhook Deliveries', data);
      }),
  },
  {
    name: 'get_webhook_delivery',
    description: 'Get details of a specific webhook delivery including request/response data',
    inputSchema: {
      type: 'object',
      properties: {
        project_id: { type: 'number', description: 'Project ID' },
        webhook_id: { type: 'number', description: 'Webhook ID' },
        delivery_id: { type: 'number', description: 'Delivery ID' },
      },
      required: ['project_id', 'webhook_id', 'delivery_id'],
    },
    handler: (args) =>
      handleToolCall(async () => {
        const client = getClient();
        const projectId = requireParam<number>(args, 'project_id');
        const webhookId = requireParam<number>(args, 'webhook_id');
        const deliveryId = requireParam<number>(args, 'delivery_id');
        const data = await client.get<Record<string, unknown>>(
          `/projects/${projectId}/webhooks/${webhookId}/deliveries/${deliveryId}`
        );
        return json('Webhook Delivery Details', data);
      }),
  },
  {
    name: 'retry_webhook_delivery',
    description: 'Retry a failed webhook delivery',
    inputSchema: {
      type: 'object',
      properties: {
        project_id: { type: 'number', description: 'Project ID' },
        webhook_id: { type: 'number', description: 'Webhook ID' },
        delivery_id: { type: 'number', description: 'Delivery ID' },
      },
      required: ['project_id', 'webhook_id', 'delivery_id'],
    },
    handler: (args) =>
      handleToolCall(async () => {
        const client = getClient();
        const projectId = requireParam<number>(args, 'project_id');
        const webhookId = requireParam<number>(args, 'webhook_id');
        const deliveryId = requireParam<number>(args, 'delivery_id');
        await client.post(
          `/projects/${projectId}/webhooks/${webhookId}/deliveries/${deliveryId}/retry`
        );
        return ok(`Webhook delivery ${deliveryId} retry initiated successfully.`);
      }),
  },
];
