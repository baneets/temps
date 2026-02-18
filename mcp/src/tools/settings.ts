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

interface Settings {
  external_url?: string;
  preview_domain?: string;
  letsencrypt?: Record<string, unknown>;
  rate_limiting?: Record<string, unknown>;
  security_headers?: Record<string, unknown>;
  [key: string]: unknown;
}

export const tools: ToolDefinition[] = [
  {
    name: 'get_settings',
    description: 'Get current platform settings',
    inputSchema: {
      type: 'object',
      properties: {},
    },
    handler: (args) =>
      handleToolCall(async () => {
        const client = getClient();
        const settings = await client.get<Settings>('/settings');

        return json('Platform Settings', settings);
      }),
  },
  {
    name: 'update_settings',
    description: 'Update platform settings (partial update)',
    inputSchema: {
      type: 'object',
      properties: {
        external_url: {
          type: 'string',
          description: 'External URL for the platform',
        },
        preview_domain: {
          type: 'string',
          description: 'Domain used for deployment previews',
        },
        letsencrypt: {
          type: 'object',
          description: "Let's Encrypt configuration",
        },
        rate_limiting: {
          type: 'object',
          description: 'Rate limiting configuration',
        },
        security_headers: {
          type: 'object',
          description: 'Security headers configuration',
        },
      },
    },
    handler: (args) =>
      handleToolCall(async () => {
        const externalUrl = optionalParam<string>(args, 'external_url');
        const previewDomain = optionalParam<string>(
          args,
          'preview_domain'
        );
        const letsencrypt = optionalParam<Record<string, unknown>>(
          args,
          'letsencrypt'
        );
        const rateLimiting = optionalParam<Record<string, unknown>>(
          args,
          'rate_limiting'
        );
        const securityHeaders = optionalParam<Record<string, unknown>>(
          args,
          'security_headers'
        );
        const client = getClient();

        const body: Record<string, unknown> = {};
        if (externalUrl !== undefined) body.external_url = externalUrl;
        if (previewDomain !== undefined)
          body.preview_domain = previewDomain;
        if (letsencrypt !== undefined) body.letsencrypt = letsencrypt;
        if (rateLimiting !== undefined)
          body.rate_limiting = rateLimiting;
        if (securityHeaders !== undefined)
          body.security_headers = securityHeaders;

        const settings = await client.put<Settings>('/settings', body);

        return json('Settings Updated', settings);
      }),
  },
];
