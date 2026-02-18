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

interface EmailDomain {
  id: number;
  domain: string;
  status: string;
  dns_records?: unknown[];
  [key: string]: unknown;
}

export const tools: ToolDefinition[] = [
  {
    name: 'list_email_domains',
    description: 'List all email domains configured on the platform',
    inputSchema: {
      type: 'object',
      properties: {},
    },
    handler: (args) =>
      handleToolCall(async () => {
        const client = getClient();
        const domains = await client.get<EmailDomain[]>('/email-domains');

        if (!domains || domains.length === 0) {
          return ok('No email domains found.');
        }

        const rows = domains.map((d) => [
          String(d.id),
          d.domain,
          d.status ?? '',
        ]);

        return ok(
          `## Email Domains (${domains.length})\n\n${table(['ID', 'Domain', 'Status'], rows)}`
        );
      }),
  },

  {
    name: 'create_email_domain',
    description: 'Add a new email domain to the platform',
    inputSchema: {
      type: 'object',
      properties: {
        domain: {
          type: 'string',
          description: 'Domain name (e.g. example.com)',
        },
        provider_id: {
          type: 'number',
          description: 'Optional email provider ID to associate with',
        },
      },
      required: ['domain'],
    },
    handler: (args) =>
      handleToolCall(async () => {
        const client = getClient();
        const domain = requireParam<string>(args, 'domain');
        const provider_id = optionalParam<number>(args, 'provider_id');

        const body: Record<string, unknown> = { domain };
        if (provider_id !== undefined) body.provider_id = provider_id;

        const result = await client.post<{ domain: EmailDomain; dns_records: unknown[] }>(
          '/email-domains',
          body
        );
        return json('Email Domain Created', result);
      }),
  },

  {
    name: 'get_email_domain',
    description: 'Get details of a specific email domain by ID',
    inputSchema: {
      type: 'object',
      properties: {
        id: {
          type: 'number',
          description: 'Email domain ID',
        },
      },
      required: ['id'],
    },
    handler: (args) =>
      handleToolCall(async () => {
        const client = getClient();
        const id = requireParam<number>(args, 'id');
        const data = await client.get<{ domain: EmailDomain; dns_records: unknown[] }>(
          `/email-domains/${id}`
        );
        return json('Email Domain Details', data);
      }),
  },

  {
    name: 'delete_email_domain',
    description: 'Delete an email domain by ID',
    inputSchema: {
      type: 'object',
      properties: {
        id: {
          type: 'number',
          description: 'Email domain ID',
        },
      },
      required: ['id'],
    },
    handler: (args) =>
      handleToolCall(async () => {
        const client = getClient();
        const id = requireParam<number>(args, 'id');
        await client.delete(`/email-domains/${id}`);
        return ok(`Email domain ${id} deleted successfully.`);
      }),
  },

  {
    name: 'get_email_domain_by_name',
    description: 'Look up an email domain by its domain name',
    inputSchema: {
      type: 'object',
      properties: {
        domain: {
          type: 'string',
          description: 'Domain name to look up (e.g. example.com)',
        },
      },
      required: ['domain'],
    },
    handler: (args) =>
      handleToolCall(async () => {
        const client = getClient();
        const domain = requireParam<string>(args, 'domain');
        const data = await client.get<{ domain: EmailDomain; dns_records: unknown[] }>(
          `/email-domains/by-domain/${domain}`
        );
        return json('Email Domain Details', data);
      }),
  },

  {
    name: 'get_email_domain_dns_records',
    description:
      'Get DNS records required for an email domain to verify and enable email sending',
    inputSchema: {
      type: 'object',
      properties: {
        id: {
          type: 'number',
          description: 'Email domain ID',
        },
      },
      required: ['id'],
    },
    handler: (args) =>
      handleToolCall(async () => {
        const client = getClient();
        const id = requireParam<number>(args, 'id');
        const records = await client.get<unknown[]>(
          `/email-domains/${id}/dns-records`
        );
        return json('DNS Records', records);
      }),
  },

  {
    name: 'setup_email_dns',
    description:
      'Automatically configure DNS records for an email domain using a linked DNS provider',
    inputSchema: {
      type: 'object',
      properties: {
        id: {
          type: 'number',
          description: 'Email domain ID',
        },
        dns_provider_id: {
          type: 'number',
          description: 'Optional DNS provider ID to use for record creation',
        },
      },
      required: ['id'],
    },
    handler: (args) =>
      handleToolCall(async () => {
        const client = getClient();
        const id = requireParam<number>(args, 'id');
        const dns_provider_id = optionalParam<number>(args, 'dns_provider_id');

        const body: Record<string, unknown> = {};
        if (dns_provider_id !== undefined) body.dns_provider_id = dns_provider_id;

        const result = await client.post<{
          success: boolean;
          message: string;
          results: unknown;
          records_created: number;
        }>(`/email-domains/${id}/setup-dns`, body);

        let text = `## DNS Setup for Email Domain ${id}\n\n`;
        text += `**Status:** ${result.success ? 'Success' : 'Failed'}\n`;
        text += `**Message:** ${result.message}\n`;
        text += `**Records Created:** ${result.records_created}\n`;

        if (result.results) {
          text += `\n\`\`\`json\n${JSON.stringify(result.results, null, 2)}\n\`\`\``;
        }

        return ok(text);
      }),
  },

  {
    name: 'verify_email_domain',
    description:
      'Trigger DNS verification for an email domain to check if records are properly configured',
    inputSchema: {
      type: 'object',
      properties: {
        id: {
          type: 'number',
          description: 'Email domain ID',
        },
      },
      required: ['id'],
    },
    handler: (args) =>
      handleToolCall(async () => {
        const client = getClient();
        const id = requireParam<number>(args, 'id');
        const result = await client.post<{ domain: EmailDomain; dns_records: unknown[] }>(
          `/email-domains/${id}/verify`
        );
        return json('Email Domain Verification Result', result);
      }),
  },
];
