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

interface EmailProvider {
  id: number;
  name: string;
  provider_type: string;
  region?: string;
  is_active?: boolean;
  [key: string]: unknown;
}

export const tools: ToolDefinition[] = [
  {
    name: 'list_email_providers',
    description: 'List all email providers configured on the platform',
    inputSchema: {
      type: 'object',
      properties: {},
    },
    handler: (args) =>
      handleToolCall(async () => {
        const client = getClient();
        const providers = await client.get<EmailProvider[]>('/email-providers');

        if (!providers || providers.length === 0) {
          return ok('No email providers found.');
        }

        const rows = providers.map((p) => [
          String(p.id),
          p.name,
          p.provider_type,
          p.region ?? '',
          String(p.is_active ?? ''),
        ]);

        return ok(
          `## Email Providers (${providers.length})\n\n${table(['ID', 'Name', 'Type', 'Region', 'Active'], rows)}`
        );
      }),
  },

  {
    name: 'create_email_provider',
    description:
      'Create a new email provider (e.g. AWS SES, Scaleway) for sending transactional emails',
    inputSchema: {
      type: 'object',
      properties: {
        name: {
          type: 'string',
          description: 'Display name for the provider',
        },
        provider_type: {
          type: 'string',
          description: 'Provider type (e.g. ses, scaleway)',
        },
        region: {
          type: 'string',
          description: 'Cloud region for the provider (e.g. us-east-1, fr-par)',
        },
        ses_credentials: {
          type: 'object',
          description:
            'AWS SES credentials (access_key_id, secret_access_key). Required if provider_type is ses.',
        },
        scaleway_credentials: {
          type: 'object',
          description:
            'Scaleway credentials (secret_key, project_id). Required if provider_type is scaleway.',
        },
      },
      required: ['name', 'provider_type', 'region'],
    },
    handler: (args) =>
      handleToolCall(async () => {
        const client = getClient();
        const name = requireParam<string>(args, 'name');
        const provider_type = requireParam<string>(args, 'provider_type');
        const region = requireParam<string>(args, 'region');
        const ses_credentials = optionalParam<Record<string, unknown>>(
          args,
          'ses_credentials'
        );
        const scaleway_credentials = optionalParam<Record<string, unknown>>(
          args,
          'scaleway_credentials'
        );

        const body: Record<string, unknown> = { name, provider_type, region };
        if (ses_credentials) body.ses_credentials = ses_credentials;
        if (scaleway_credentials)
          body.scaleway_credentials = scaleway_credentials;

        const provider = await client.post<EmailProvider>(
          '/email-providers',
          body
        );
        return json('Email Provider Created', provider);
      }),
  },

  {
    name: 'get_email_provider',
    description: 'Get details of a specific email provider by ID',
    inputSchema: {
      type: 'object',
      properties: {
        id: {
          type: 'number',
          description: 'Email provider ID',
        },
      },
      required: ['id'],
    },
    handler: (args) =>
      handleToolCall(async () => {
        const client = getClient();
        const id = requireParam<number>(args, 'id');
        const provider = await client.get<EmailProvider>(
          `/email-providers/${id}`
        );
        return json('Email Provider Details', provider);
      }),
  },

  {
    name: 'delete_email_provider',
    description: 'Delete an email provider by ID',
    inputSchema: {
      type: 'object',
      properties: {
        id: {
          type: 'number',
          description: 'Email provider ID',
        },
      },
      required: ['id'],
    },
    handler: (args) =>
      handleToolCall(async () => {
        const client = getClient();
        const id = requireParam<number>(args, 'id');
        await client.delete(`/email-providers/${id}`);
        return ok(`Email provider ${id} deleted successfully.`);
      }),
  },

  {
    name: 'test_email_provider',
    description:
      'Send a test email through a provider to verify it is correctly configured',
    inputSchema: {
      type: 'object',
      properties: {
        id: {
          type: 'number',
          description: 'Email provider ID',
        },
        from: {
          type: 'string',
          description: 'Sender email address to test with',
        },
        from_name: {
          type: 'string',
          description: 'Optional sender display name',
        },
      },
      required: ['id', 'from'],
    },
    handler: (args) =>
      handleToolCall(async () => {
        const client = getClient();
        const id = requireParam<number>(args, 'id');
        const from = requireParam<string>(args, 'from');
        const from_name = optionalParam<string>(args, 'from_name');

        const body: Record<string, unknown> = { from };
        if (from_name) body.from_name = from_name;

        const result = await client.post<Record<string, unknown>>(
          `/email-providers/${id}/test`,
          body
        );
        return ok(
          `Test email sent successfully via provider ${id}.\n\n\`\`\`json\n${JSON.stringify(result, null, 2)}\n\`\`\``
        );
      }),
  },
];
