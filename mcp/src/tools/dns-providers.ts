import { getClient } from '../api/index.js';
import { ok, json, table, formatDate, handleToolCall, requireParam, optionalParam } from './_helpers.js';
import type { ToolDefinition } from '../types/index.js';

export const tools: ToolDefinition[] = [
  {
    name: 'list_dns_providers',
    description: 'List all configured DNS providers',
    inputSchema: {
      type: 'object',
      properties: {},
    },
    handler: (args) =>
      handleToolCall(async () => {
        const client = getClient();
        const data = await client.get<Array<Record<string, unknown>>>('/dns-providers');
        const providers = Array.isArray(data) ? data : [];

        if (providers.length === 0) {
          return ok('No DNS providers configured.');
        }

        const rows = providers.map((p) => [
          String(p.id ?? ''),
          String(p.name ?? ''),
          String(p.provider_type ?? ''),
          String(p.is_active ?? ''),
        ]);

        return ok(
          `## DNS Providers (${providers.length})\n\n${table(['ID', 'Name', 'Type', 'Active'], rows)}`
        );
      }),
  },
  {
    name: 'create_dns_provider',
    description: 'Register a new DNS provider (e.g. Cloudflare, Route53)',
    inputSchema: {
      type: 'object',
      properties: {
        name: { type: 'string', description: 'Display name for the provider' },
        provider_type: { type: 'string', description: 'Provider type (e.g. cloudflare, route53, digitalocean)' },
        credentials: {
          type: 'object',
          description: 'Provider-specific credentials (e.g. { api_token: "..." })',
        },
        description: { type: 'string', description: 'Optional description' },
      },
      required: ['name', 'provider_type', 'credentials'],
    },
    handler: (args) =>
      handleToolCall(async () => {
        const client = getClient();
        const name = requireParam<string>(args, 'name');
        const providerType = requireParam<string>(args, 'provider_type');
        const credentials = requireParam<Record<string, unknown>>(args, 'credentials');
        const description = optionalParam<string>(args, 'description');

        const body: Record<string, unknown> = {
          name,
          provider_type: providerType,
          credentials,
        };
        if (description !== undefined) body.description = description;

        const result = await client.post<Record<string, unknown>>('/dns-providers', body);
        return json('DNS Provider Created', result);
      }),
  },
  {
    name: 'get_dns_provider',
    description: 'Get details of a specific DNS provider',
    inputSchema: {
      type: 'object',
      properties: {
        id: { type: 'number', description: 'DNS provider ID' },
      },
      required: ['id'],
    },
    handler: (args) =>
      handleToolCall(async () => {
        const client = getClient();
        const id = requireParam<number>(args, 'id');
        const data = await client.get<Record<string, unknown>>(`/dns-providers/${id}`);
        return json('DNS Provider Details', data);
      }),
  },
  {
    name: 'update_dns_provider',
    description: 'Update a DNS provider configuration',
    inputSchema: {
      type: 'object',
      properties: {
        id: { type: 'number', description: 'DNS provider ID' },
        name: { type: 'string', description: 'Updated display name' },
        description: { type: 'string', description: 'Updated description' },
        credentials: {
          type: 'object',
          description: 'Updated provider credentials',
        },
        is_active: { type: 'boolean', description: 'Whether the provider is active' },
      },
      required: ['id'],
    },
    handler: (args) =>
      handleToolCall(async () => {
        const client = getClient();
        const id = requireParam<number>(args, 'id');
        const name = optionalParam<string>(args, 'name');
        const description = optionalParam<string>(args, 'description');
        const credentials = optionalParam<Record<string, unknown>>(args, 'credentials');
        const isActive = optionalParam<boolean>(args, 'is_active');

        const body: Record<string, unknown> = {};
        if (name !== undefined) body.name = name;
        if (description !== undefined) body.description = description;
        if (credentials !== undefined) body.credentials = credentials;
        if (isActive !== undefined) body.is_active = isActive;

        const result = await client.put<Record<string, unknown>>(`/dns-providers/${id}`, body);
        return json('DNS Provider Updated', result);
      }),
  },
  {
    name: 'delete_dns_provider',
    description: 'Delete a DNS provider',
    inputSchema: {
      type: 'object',
      properties: {
        id: { type: 'number', description: 'DNS provider ID' },
      },
      required: ['id'],
    },
    handler: (args) =>
      handleToolCall(async () => {
        const client = getClient();
        const id = requireParam<number>(args, 'id');
        await client.delete(`/dns-providers/${id}`);
        return ok(`DNS provider ${id} deleted successfully.`);
      }),
  },
  {
    name: 'test_dns_provider',
    description: 'Test connectivity and credentials for a DNS provider',
    inputSchema: {
      type: 'object',
      properties: {
        id: { type: 'number', description: 'DNS provider ID' },
      },
      required: ['id'],
    },
    handler: (args) =>
      handleToolCall(async () => {
        const client = getClient();
        const id = requireParam<number>(args, 'id');
        const result = await client.post<{ success: boolean; message: string }>(
          `/dns-providers/${id}/test`
        );
        const status = result.success ? 'passed' : 'failed';
        return ok(`## DNS Provider Test ${status}\n\n**Result:** ${result.success ? 'Success' : 'Failed'}\n**Message:** ${result.message}`);
      }),
  },
  {
    name: 'list_dns_provider_zones',
    description: 'List DNS zones available from a provider',
    inputSchema: {
      type: 'object',
      properties: {
        id: { type: 'number', description: 'DNS provider ID' },
      },
      required: ['id'],
    },
    handler: (args) =>
      handleToolCall(async () => {
        const client = getClient();
        const id = requireParam<number>(args, 'id');
        const data = await client.get<unknown>(`/dns-providers/${id}/zones`);
        return json('DNS Zones', data);
      }),
  },
  {
    name: 'list_managed_domains',
    description: 'List domains managed by a DNS provider',
    inputSchema: {
      type: 'object',
      properties: {
        id: { type: 'number', description: 'DNS provider ID' },
      },
      required: ['id'],
    },
    handler: (args) =>
      handleToolCall(async () => {
        const client = getClient();
        const id = requireParam<number>(args, 'id');
        const data = await client.get<unknown>(`/dns-providers/${id}/domains`);
        return json('Managed Domains', data);
      }),
  },
  {
    name: 'add_managed_domain',
    description: 'Add a domain to be managed by a DNS provider',
    inputSchema: {
      type: 'object',
      properties: {
        id: { type: 'number', description: 'DNS provider ID' },
        domain: { type: 'string', description: 'Domain name to manage' },
        auto_manage: { type: 'boolean', description: 'Whether to automatically manage DNS records' },
      },
      required: ['id', 'domain'],
    },
    handler: (args) =>
      handleToolCall(async () => {
        const client = getClient();
        const id = requireParam<number>(args, 'id');
        const domain = requireParam<string>(args, 'domain');
        const autoManage = optionalParam<boolean>(args, 'auto_manage');

        const body: Record<string, unknown> = { domain };
        if (autoManage !== undefined) body.auto_manage = autoManage;

        const result = await client.post<Record<string, unknown>>(
          `/dns-providers/${id}/domains`,
          body
        );
        return json('Managed Domain Added', result);
      }),
  },
  {
    name: 'remove_managed_domain',
    description: 'Remove a domain from a DNS provider',
    inputSchema: {
      type: 'object',
      properties: {
        provider_id: { type: 'number', description: 'DNS provider ID' },
        domain: { type: 'string', description: 'Domain name to remove' },
      },
      required: ['provider_id', 'domain'],
    },
    handler: (args) =>
      handleToolCall(async () => {
        const client = getClient();
        const providerId = requireParam<number>(args, 'provider_id');
        const domain = requireParam<string>(args, 'domain');
        await client.delete(`/dns-providers/${providerId}/domains/${domain}`);
        return ok(`Domain '${domain}' removed from provider ${providerId} successfully.`);
      }),
  },
  {
    name: 'verify_managed_domain',
    description: 'Verify DNS configuration for a managed domain',
    inputSchema: {
      type: 'object',
      properties: {
        provider_id: { type: 'number', description: 'DNS provider ID' },
        domain: { type: 'string', description: 'Domain name to verify' },
      },
      required: ['provider_id', 'domain'],
    },
    handler: (args) =>
      handleToolCall(async () => {
        const client = getClient();
        const providerId = requireParam<number>(args, 'provider_id');
        const domain = requireParam<string>(args, 'domain');
        const result = await client.post<Record<string, unknown>>(
          `/dns-providers/${providerId}/domains/${domain}/verify`
        );
        return json('Domain Verification Result', result);
      }),
  },
  {
    name: 'lookup_dns_a_records',
    description: 'Look up DNS A records for a domain',
    inputSchema: {
      type: 'object',
      properties: {
        domain: { type: 'string', description: 'Domain name to look up' },
      },
      required: ['domain'],
    },
    handler: (args) =>
      handleToolCall(async () => {
        const client = getClient();
        const domain = requireParam<string>(args, 'domain');
        const data = await client.get<{ domain: string; records: unknown[]; count: number; dns_servers: unknown }>(
          '/dns/lookup',
          { domain }
        );
        return json(`DNS A Records for ${data.domain}`, data);
      }),
  },
];
