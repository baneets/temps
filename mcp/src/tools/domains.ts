/**
 * Domain management tools
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
    name: 'list_domains',
    description: 'List all domains configured in the platform',
    inputSchema: {
      type: 'object',
      properties: {},
    },
    handler: (args) =>
      handleToolCall(async () => {
        const client = getClient();
        const result = await client.get<{ domains: any[] }>('/domains');
        const domains = result.domains ?? [];
        const rows = domains.map((d: any) => [
          String(d.id ?? ''),
          d.domain ?? '',
          d.status ?? '',
          d.challenge_type ?? '',
          formatDate(d.created_at),
        ]);
        return ok(
          `## Domains\n\n${table(
            ['ID', 'Domain', 'Status', 'Challenge Type', 'Created'],
            rows
          )}`
        );
      }),
  },

  {
    name: 'add_domain',
    description: 'Add a new domain to the platform',
    inputSchema: {
      type: 'object',
      properties: {
        domain: {
          type: 'string',
          description: 'The domain name (e.g. "app.example.com")',
        },
        challenge_type: {
          type: 'string',
          description:
            'TLS challenge type (e.g. "http-01", "dns-01"). Defaults to server setting if omitted.',
        },
      },
      required: ['domain'],
    },
    handler: (args) =>
      handleToolCall(async () => {
        const client = getClient();
        const domain = requireParam<string>(args, 'domain');
        const challengeType = optionalParam<string>(args, 'challenge_type');

        const body: Record<string, unknown> = { domain };
        if (challengeType !== undefined) body.challenge_type = challengeType;

        const result = await client.post<any>('/domains', body);
        return json('Domain Added', result);
      }),
  },

  {
    name: 'verify_domain',
    description:
      'Verify and provision TLS for a domain. Triggers certificate issuance after challenge validation.',
    inputSchema: {
      type: 'object',
      properties: {
        domain: {
          type: 'string',
          description: 'The domain name to verify',
        },
      },
      required: ['domain'],
    },
    handler: (args) =>
      handleToolCall(async () => {
        const client = getClient();
        const domain = requireParam<string>(args, 'domain');
        const result = await client.post<any>(
          `/domains/${encodeURIComponent(domain)}/provision`
        );
        return json('Domain Verification Result', result);
      }),
  },

  {
    name: 'remove_domain',
    description: 'Remove a domain from the platform',
    inputSchema: {
      type: 'object',
      properties: {
        domain: {
          type: 'string',
          description: 'The domain name to remove',
        },
      },
      required: ['domain'],
    },
    handler: (args) =>
      handleToolCall(async () => {
        const client = getClient();
        const domain = requireParam<string>(args, 'domain');
        await client.delete(
          `/domains/${encodeURIComponent(domain)}`
        );
        return ok(`Domain "${domain}" removed.`);
      }),
  },

  {
    name: 'get_domain_status',
    description: 'Get the current status and TLS details of a domain',
    inputSchema: {
      type: 'object',
      properties: {
        domain_id: {
          type: 'string',
          description: 'The domain name (e.g. "app.example.com")',
        },
      },
      required: ['domain_id'],
    },
    handler: (args) =>
      handleToolCall(async () => {
        const client = getClient();
        const domain = requireParam<string>(args, 'domain_id');
        const result = await client.get<any>(
          `/domains/${encodeURIComponent(domain)}/status`
        );
        return json('Domain Status', result);
      }),
  },

  {
    name: 'renew_domain_ssl',
    description: 'Trigger SSL/TLS certificate renewal for a domain',
    inputSchema: {
      type: 'object',
      properties: {
        domain: {
          type: 'string',
          description: 'The domain name to renew',
        },
      },
      required: ['domain'],
    },
    handler: (args) =>
      handleToolCall(async () => {
        const client = getClient();
        const domain = requireParam<string>(args, 'domain');
        const result = await client.post<any>(
          `/domains/${encodeURIComponent(domain)}/renew`
        );
        return json('SSL Renewal Result', result);
      }),
  },

  {
    name: 'list_domain_orders',
    description:
      'List all pending and completed ACME certificate orders',
    inputSchema: {
      type: 'object',
      properties: {},
    },
    handler: (args) =>
      handleToolCall(async () => {
        const client = getClient();
        const result = await client.get<{ orders: any[] }>(
          '/orders'
        );
        const orders = result.orders ?? [];
        const rows = orders.map((o: any) => [
          String(o.id ?? ''),
          o.domain ?? '',
          o.status ?? '',
          o.challenge_type ?? '',
          formatDate(o.created_at),
        ]);
        return ok(
          `## Domain Orders\n\n${table(
            ['ID', 'Domain', 'Status', 'Challenge Type', 'Created'],
            rows
          )}`
        );
      }),
  },

  {
    name: 'get_domain_order',
    description: 'Get details of a specific ACME certificate order',
    inputSchema: {
      type: 'object',
      properties: {
        domain_id: {
          type: 'number',
          description: 'The domain ID',
        },
      },
      required: ['domain_id'],
    },
    handler: (args) =>
      handleToolCall(async () => {
        const client = getClient();
        const domainId = requireParam<number>(args, 'domain_id');
        const result = await client.get<any>(
          `/domains/${domainId}/order`
        );
        return json('Domain Order', result);
      }),
  },

  {
    name: 'create_domain_order',
    description:
      'Create a new ACME certificate order for a domain. Returns TXT records if using DNS challenge.',
    inputSchema: {
      type: 'object',
      properties: {
        domain_id: {
          type: 'number',
          description: 'The domain ID',
        },
      },
      required: ['domain_id'],
    },
    handler: (args) =>
      handleToolCall(async () => {
        const client = getClient();
        const domainId = requireParam<number>(args, 'domain_id');
        const result = await client.post<any>(
          `/domains/${domainId}/order`
        );
        return json('Domain Order Created', result);
      }),
  },

  {
    name: 'finalize_domain_order',
    description:
      'Finalize a pending ACME certificate order after challenges have been satisfied',
    inputSchema: {
      type: 'object',
      properties: {
        domain_id: {
          type: 'number',
          description: 'The domain ID',
        },
      },
      required: ['domain_id'],
    },
    handler: (args) =>
      handleToolCall(async () => {
        const client = getClient();
        const domainId = requireParam<number>(args, 'domain_id');
        const result = await client.post<any>(
          `/domains/${domainId}/order/finalize`
        );
        return json('Domain Order Finalized', result);
      }),
  },

  {
    name: 'cancel_domain_order',
    description: 'Cancel a pending ACME certificate order',
    inputSchema: {
      type: 'object',
      properties: {
        domain_id: {
          type: 'number',
          description: 'The domain ID',
        },
      },
      required: ['domain_id'],
    },
    handler: (args) =>
      handleToolCall(async () => {
        const client = getClient();
        const domainId = requireParam<number>(args, 'domain_id');
        await client.delete(
          `/domains/${domainId}/order`
        );
        return ok(`Domain order for domain ${domainId} cancelled.`);
      }),
  },

  {
    name: 'setup_dns_challenge',
    description:
      'Set up DNS challenge records for a domain using a configured DNS provider',
    inputSchema: {
      type: 'object',
      properties: {
        domain_id: {
          type: 'number',
          description: 'The domain ID',
        },
        dns_provider_id: {
          type: 'number',
          description: 'The DNS provider ID to use for creating records',
        },
      },
      required: ['domain_id', 'dns_provider_id'],
    },
    handler: (args) =>
      handleToolCall(async () => {
        const client = getClient();
        const domainId = requireParam<number>(args, 'domain_id');
        const dnsProviderId = requireParam<number>(args, 'dns_provider_id');
        const result = await client.post<any>(
          `/domains/${domainId}/setup-dns`,
          { dns_provider_id: dnsProviderId }
        );
        return json('DNS Challenge Setup', result);
      }),
  },

  {
    name: 'get_http_challenge_debug',
    description:
      'Get debug information for HTTP-01 challenge validation for a domain',
    inputSchema: {
      type: 'object',
      properties: {
        domain: {
          type: 'string',
          description: 'The domain name to debug',
        },
      },
      required: ['domain'],
    },
    handler: (args) =>
      handleToolCall(async () => {
        const client = getClient();
        const domain = requireParam<string>(args, 'domain');
        const result = await client.get<any>(
          `/domains/${encodeURIComponent(domain)}/http-challenge-debug`
        );
        return json('HTTP Challenge Debug Info', result);
      }),
  },
];
