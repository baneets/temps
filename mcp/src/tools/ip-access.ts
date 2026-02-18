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

interface IpAccessRule {
  id: number;
  ip_address: string;
  action: string;
  reason?: string;
  created_at?: string;
  updated_at?: string;
}

export const tools: ToolDefinition[] = [
  {
    name: 'list_ip_access_rules',
    description: 'List all IP access rules (allow/block list)',
    inputSchema: {
      type: 'object',
      properties: {},
    },
    handler: (args) =>
      handleToolCall(async () => {
        const client = getClient();
        const rules = await client.get<IpAccessRule[]>('/ip-access-control');

        if (!rules || rules.length === 0) {
          return ok('No IP access rules found.');
        }

        const rows = rules.map((r) => [
          String(r.id),
          r.ip_address ?? '',
          r.action ?? '',
          r.reason ?? '',
        ]);

        return ok(
          `## IP Access Rules (${rules.length})\n\n${table(
            ['ID', 'IP Address', 'Action', 'Reason'],
            rows
          )}`
        );
      }),
  },

  {
    name: 'create_ip_access_rule',
    description: 'Create a new IP access rule to allow or block an IP address',
    inputSchema: {
      type: 'object',
      properties: {
        ip_address: {
          type: 'string',
          description: 'IP address or CIDR range',
        },
        action: {
          type: 'string',
          description: 'Action to take (e.g. "allow" or "block")',
        },
        reason: {
          type: 'string',
          description: 'Optional reason for the rule',
        },
      },
      required: ['ip_address', 'action'],
    },
    handler: (args) =>
      handleToolCall(async () => {
        const client = getClient();
        const ipAddress = requireParam<string>(args, 'ip_address');
        const action = requireParam<string>(args, 'action');
        const reason = optionalParam<string>(args, 'reason');

        const body: Record<string, unknown> = { ip_address: ipAddress, action };
        if (reason !== undefined) body.reason = reason;

        const rule = await client.post<IpAccessRule>('/ip-access-control', body);
        return json('IP Access Rule Created', rule);
      }),
  },

  {
    name: 'get_ip_access_rule',
    description: 'Get details of a specific IP access rule',
    inputSchema: {
      type: 'object',
      properties: {
        id: {
          type: 'number',
          description: 'IP access rule ID',
        },
      },
      required: ['id'],
    },
    handler: (args) =>
      handleToolCall(async () => {
        const client = getClient();
        const id = requireParam<number>(args, 'id');
        const rule = await client.get<IpAccessRule>(`/ip-access-control/${id}`);
        return json('IP Access Rule Details', rule);
      }),
  },

  {
    name: 'update_ip_access_rule',
    description: 'Update an existing IP access rule',
    inputSchema: {
      type: 'object',
      properties: {
        id: {
          type: 'number',
          description: 'IP access rule ID',
        },
        ip_address: {
          type: 'string',
          description: 'Updated IP address or CIDR range',
        },
        action: {
          type: 'string',
          description: 'Updated action (e.g. "allow" or "block")',
        },
        reason: {
          type: 'string',
          description: 'Updated reason for the rule',
        },
      },
      required: ['id'],
    },
    handler: (args) =>
      handleToolCall(async () => {
        const client = getClient();
        const id = requireParam<number>(args, 'id');
        const ipAddress = optionalParam<string>(args, 'ip_address');
        const action = optionalParam<string>(args, 'action');
        const reason = optionalParam<string>(args, 'reason');

        const body: Record<string, unknown> = {};
        if (ipAddress !== undefined) body.ip_address = ipAddress;
        if (action !== undefined) body.action = action;
        if (reason !== undefined) body.reason = reason;

        const rule = await client.patch<IpAccessRule>(`/ip-access-control/${id}`, body);
        return json('IP Access Rule Updated', rule);
      }),
  },

  {
    name: 'delete_ip_access_rule',
    description: 'Delete an IP access rule',
    inputSchema: {
      type: 'object',
      properties: {
        id: {
          type: 'number',
          description: 'IP access rule ID',
        },
      },
      required: ['id'],
    },
    handler: (args) =>
      handleToolCall(async () => {
        const client = getClient();
        const id = requireParam<number>(args, 'id');
        await client.delete(`/ip-access-control/${id}`);
        return ok(`IP access rule ${id} deleted successfully.`);
      }),
  },

  {
    name: 'check_ip_blocked',
    description: 'Check whether a specific IP address is blocked',
    inputSchema: {
      type: 'object',
      properties: {
        ip: {
          type: 'string',
          description: 'IP address to check',
        },
      },
      required: ['ip'],
    },
    handler: (args) =>
      handleToolCall(async () => {
        const client = getClient();
        const ip = requireParam<string>(args, 'ip');
        const data = await client.get<Record<string, unknown>>(
          `/ip-access-control/check/${ip}`
        );
        return json(`IP Block Status for ${ip}`, data);
      }),
  },
];
