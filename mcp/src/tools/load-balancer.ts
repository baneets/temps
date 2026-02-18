import { getClient } from '../api/index.js';
import {
  ok,
  json,
  table,
  handleToolCall,
  requireParam,
  optionalParam,
} from './_helpers.js';
import type { ToolDefinition } from '../types/index.js';

interface LbRoute {
  domain: string;
  host?: string;
  port?: number;
  target?: string;
  status?: string;
  enabled?: boolean;
  ssl_enabled?: boolean;
  [key: string]: unknown;
}

export const tools: ToolDefinition[] = [
  {
    name: 'list_lb_routes',
    description: 'List all load balancer routes',
    inputSchema: {
      type: 'object',
      properties: {},
    },
    handler: (args) =>
      handleToolCall(async () => {
        const client = getClient();
        const routes = await client.get<LbRoute[]>('/lb/routes');

        if (!routes || routes.length === 0) {
          return ok('No load balancer routes found.');
        }

        const rows = routes.map((r) => [
          r.domain,
          r.target ?? `${r.host ?? ''}:${r.port ?? ''}`,
          r.status ?? (r.enabled !== false ? 'active' : 'disabled'),
          String(r.ssl_enabled ?? false),
        ]);

        return ok(
          `## Load Balancer Routes (${routes.length})\n\n${table(['Domain', 'Target', 'Status', 'SSL'], rows)}`
        );
      }),
  },

  {
    name: 'create_lb_route',
    description:
      'Create a new load balancer route to forward traffic from a domain to a backend host and port',
    inputSchema: {
      type: 'object',
      properties: {
        domain: {
          type: 'string',
          description: 'Domain name for the route (e.g. app.example.com)',
        },
        host: {
          type: 'string',
          description: 'Backend host to route traffic to',
        },
        port: {
          type: 'number',
          description: 'Backend port to route traffic to',
        },
      },
      required: ['domain', 'host', 'port'],
    },
    handler: (args) =>
      handleToolCall(async () => {
        const client = getClient();
        const domain = requireParam<string>(args, 'domain');
        const host = requireParam<string>(args, 'host');
        const port = requireParam<number>(args, 'port');

        const route = await client.post<LbRoute>('/lb/routes', {
          domain,
          host,
          port,
        });
        return json('Load Balancer Route Created', route);
      }),
  },

  {
    name: 'get_lb_route',
    description: 'Get details of a specific load balancer route by domain',
    inputSchema: {
      type: 'object',
      properties: {
        domain: {
          type: 'string',
          description: 'Domain name of the route',
        },
      },
      required: ['domain'],
    },
    handler: (args) =>
      handleToolCall(async () => {
        const client = getClient();
        const domain = requireParam<string>(args, 'domain');
        const route = await client.get<LbRoute>(
          `/lb/routes/${domain}`
        );
        return json('Load Balancer Route Details', route);
      }),
  },

  {
    name: 'update_lb_route',
    description:
      'Update an existing load balancer route (enable/disable, change backend host or port)',
    inputSchema: {
      type: 'object',
      properties: {
        domain: {
          type: 'string',
          description: 'Domain name of the route to update',
        },
        enabled: {
          type: 'boolean',
          description: 'Whether the route is enabled',
        },
        host: {
          type: 'string',
          description: 'New backend host',
        },
        port: {
          type: 'number',
          description: 'New backend port',
        },
      },
      required: ['domain'],
    },
    handler: (args) =>
      handleToolCall(async () => {
        const client = getClient();
        const domain = requireParam<string>(args, 'domain');
        const enabled = optionalParam<boolean>(args, 'enabled');
        const host = optionalParam<string>(args, 'host');
        const port = optionalParam<number>(args, 'port');

        const body: Record<string, unknown> = {};
        if (enabled !== undefined) body.enabled = enabled;
        if (host !== undefined) body.host = host;
        if (port !== undefined) body.port = port;

        const route = await client.put<LbRoute>(
          `/lb/routes/${domain}`,
          body
        );
        return json('Load Balancer Route Updated', route);
      }),
  },

  {
    name: 'delete_lb_route',
    description: 'Delete a load balancer route by domain',
    inputSchema: {
      type: 'object',
      properties: {
        domain: {
          type: 'string',
          description: 'Domain name of the route to delete',
        },
      },
      required: ['domain'],
    },
    handler: (args) =>
      handleToolCall(async () => {
        const client = getClient();
        const domain = requireParam<string>(args, 'domain');
        await client.delete(`/lb/routes/${domain}`);
        return ok(`Load balancer route for '${domain}' deleted successfully.`);
      }),
  },
];
