import { getClient } from '../api/index.js';
import { ok, json, handleToolCall } from './_helpers.js';
import type { ToolDefinition } from '../types/index.js';

export const tools: ToolDefinition[] = [
  {
    name: 'get_platform_info',
    description:
      'Get general platform information including version, features, and capabilities',
    inputSchema: {
      type: 'object',
      properties: {},
    },
    handler: (args) =>
      handleToolCall(async () => {
        const client = getClient();
        const data = await client.get<Record<string, unknown>>(
          '/.well-known/temps.json'
        );
        return json('Platform Info', data);
      }),
  },

  {
    name: 'get_platform_access',
    description:
      'Get platform access mode and capabilities for the current user',
    inputSchema: {
      type: 'object',
      properties: {},
    },
    handler: (args) =>
      handleToolCall(async () => {
        const client = getClient();
        const data = await client.get<Record<string, unknown>>(
          '/platform/access-info'
        );
        return json('Platform Access', data);
      }),
  },

  {
    name: 'get_platform_private_ip',
    description: 'Get the private IP address of the platform server',
    inputSchema: {
      type: 'object',
      properties: {},
    },
    handler: (args) =>
      handleToolCall(async () => {
        const client = getClient();
        const data = await client.get<{ ip: string }>(
          '/platform/private-ip'
        );
        return ok(`Private IP: ${data.ip}`);
      }),
  },

  {
    name: 'get_platform_public_ip',
    description: 'Get the public IP address of the platform server',
    inputSchema: {
      type: 'object',
      properties: {},
    },
    handler: (args) =>
      handleToolCall(async () => {
        const client = getClient();
        const data = await client.get<{ ip: string }>(
          '/platform/public-ip'
        );
        return ok(`Public IP: ${data.ip}`);
      }),
  },
];
