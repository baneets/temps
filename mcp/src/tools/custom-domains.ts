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

interface CustomDomain {
  id: number;
  domain: string;
  environment_id: number;
  branch?: string;
  redirect_to?: string;
  status_code?: number;
  certificate_id?: number;
  status?: string;
  created_at?: string;
  updated_at?: string;
}

interface CustomDomainsResponse {
  domains: CustomDomain[];
}

export const tools: ToolDefinition[] = [
  {
    name: 'list_custom_domains',
    description: 'List all custom domains for a project',
    inputSchema: {
      type: 'object',
      properties: {
        project_id: {
          type: 'number',
          description: 'Project ID',
        },
      },
      required: ['project_id'],
    },
    handler: (args) =>
      handleToolCall(async () => {
        const projectId = requireParam<number>(args, 'project_id');
        const client = getClient();
        const response = await client.get<CustomDomainsResponse>(
          `/projects/${projectId}/custom-domains`
        );

        const domains = response.domains;
        if (!domains || domains.length === 0) {
          return ok('No custom domains found for this project.');
        }

        const rows = domains.map((d) => [
          String(d.id),
          d.domain,
          String(d.environment_id),
          d.status ?? 'N/A',
          formatDate(d.created_at),
        ]);

        return ok(
          table(
            ['ID', 'Domain', 'Environment ID', 'Status', 'Created'],
            rows
          )
        );
      }),
  },
  {
    name: 'create_custom_domain',
    description: 'Add a custom domain to a project',
    inputSchema: {
      type: 'object',
      properties: {
        project_id: {
          type: 'number',
          description: 'Project ID',
        },
        domain: {
          type: 'string',
          description: 'Domain name (e.g. app.example.com)',
        },
        environment_id: {
          type: 'number',
          description: 'Environment ID to link the domain to',
        },
        branch: {
          type: 'string',
          description: 'Optional branch to serve from this domain',
        },
        redirect_to: {
          type: 'string',
          description: 'Optional URL to redirect this domain to',
        },
        status_code: {
          type: 'number',
          description:
            'HTTP status code for redirect (e.g. 301, 302)',
        },
      },
      required: ['project_id', 'domain', 'environment_id'],
    },
    handler: (args) =>
      handleToolCall(async () => {
        const projectId = requireParam<number>(args, 'project_id');
        const domain = requireParam<string>(args, 'domain');
        const environmentId = requireParam<number>(args, 'environment_id');
        const branch = optionalParam<string>(args, 'branch');
        const redirectTo = optionalParam<string>(args, 'redirect_to');
        const statusCode = optionalParam<number>(args, 'status_code');
        const client = getClient();

        const body: Record<string, unknown> = {
          domain,
          environment_id: environmentId,
        };
        if (branch !== undefined) body.branch = branch;
        if (redirectTo !== undefined) body.redirect_to = redirectTo;
        if (statusCode !== undefined) body.status_code = statusCode;

        const created = await client.post<CustomDomain>(
          `/projects/${projectId}/custom-domains`,
          body
        );

        return json('Custom Domain Created', created);
      }),
  },
  {
    name: 'get_custom_domain',
    description: 'Get details of a specific custom domain',
    inputSchema: {
      type: 'object',
      properties: {
        project_id: {
          type: 'number',
          description: 'Project ID',
        },
        domain_id: {
          type: 'number',
          description: 'Custom domain ID',
        },
      },
      required: ['project_id', 'domain_id'],
    },
    handler: (args) =>
      handleToolCall(async () => {
        const projectId = requireParam<number>(args, 'project_id');
        const domainId = requireParam<number>(args, 'domain_id');
        const client = getClient();
        const domain = await client.get<CustomDomain>(
          `/projects/${projectId}/custom-domains/${domainId}`
        );

        return json('Custom Domain Details', domain);
      }),
  },
  {
    name: 'update_custom_domain',
    description: 'Update a custom domain configuration',
    inputSchema: {
      type: 'object',
      properties: {
        project_id: {
          type: 'number',
          description: 'Project ID',
        },
        domain_id: {
          type: 'number',
          description: 'Custom domain ID',
        },
        domain: {
          type: 'string',
          description: 'New domain name',
        },
        environment_id: {
          type: 'number',
          description: 'New environment ID',
        },
        branch: {
          type: 'string',
          description: 'Branch to serve from this domain',
        },
        redirect_to: {
          type: 'string',
          description: 'URL to redirect this domain to',
        },
        status_code: {
          type: 'number',
          description: 'HTTP status code for redirect',
        },
      },
      required: ['project_id', 'domain_id'],
    },
    handler: (args) =>
      handleToolCall(async () => {
        const projectId = requireParam<number>(args, 'project_id');
        const domainId = requireParam<number>(args, 'domain_id');
        const domain = optionalParam<string>(args, 'domain');
        const environmentId = optionalParam<number>(args, 'environment_id');
        const branch = optionalParam<string>(args, 'branch');
        const redirectTo = optionalParam<string>(args, 'redirect_to');
        const statusCode = optionalParam<number>(args, 'status_code');
        const client = getClient();

        const body: Record<string, unknown> = {};
        if (domain !== undefined) body.domain = domain;
        if (environmentId !== undefined) body.environment_id = environmentId;
        if (branch !== undefined) body.branch = branch;
        if (redirectTo !== undefined) body.redirect_to = redirectTo;
        if (statusCode !== undefined) body.status_code = statusCode;

        const updated = await client.put<CustomDomain>(
          `/projects/${projectId}/custom-domains/${domainId}`,
          body
        );

        return json('Custom Domain Updated', updated);
      }),
  },
  {
    name: 'delete_custom_domain',
    description: 'Delete a custom domain from a project',
    inputSchema: {
      type: 'object',
      properties: {
        project_id: {
          type: 'number',
          description: 'Project ID',
        },
        domain_id: {
          type: 'number',
          description: 'Custom domain ID',
        },
      },
      required: ['project_id', 'domain_id'],
    },
    handler: (args) =>
      handleToolCall(async () => {
        const projectId = requireParam<number>(args, 'project_id');
        const domainId = requireParam<number>(args, 'domain_id');
        const client = getClient();
        await client.delete(
          `/projects/${projectId}/custom-domains/${domainId}`
        );

        return ok(`Custom domain ${domainId} deleted successfully.`);
      }),
  },
  {
    name: 'link_custom_domain_to_certificate',
    description: 'Link a custom domain to a TLS certificate',
    inputSchema: {
      type: 'object',
      properties: {
        project_id: {
          type: 'number',
          description: 'Project ID',
        },
        domain_id: {
          type: 'number',
          description: 'Custom domain ID',
        },
        certificate_id: {
          type: 'number',
          description: 'TLS certificate ID to link',
        },
      },
      required: ['project_id', 'domain_id', 'certificate_id'],
    },
    handler: (args) =>
      handleToolCall(async () => {
        const projectId = requireParam<number>(args, 'project_id');
        const domainId = requireParam<number>(args, 'domain_id');
        const certificateId = requireParam<number>(args, 'certificate_id');
        const client = getClient();

        await client.post(
          `/projects/${projectId}/custom-domains/${domainId}/link-certificate/${certificateId}`
        );

        return ok(
          `Custom domain ${domainId} linked to certificate ${certificateId} successfully.`
        );
      }),
  },
];
