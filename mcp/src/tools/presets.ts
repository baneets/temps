import { getClient } from '../api/index.js';
import {
  ok,
  json,
  table,
  handleToolCall,
  requireParam,
} from './_helpers.js';
import type { ToolDefinition } from '../types/index.js';

interface Preset {
  slug: string;
  name: string;
  type: string;
  description?: string;
  [key: string]: unknown;
}

export const tools: ToolDefinition[] = [
  {
    name: 'list_presets',
    description: 'List all available deployment presets',
    inputSchema: {
      type: 'object',
      properties: {},
    },
    handler: (args) =>
      handleToolCall(async () => {
        const client = getClient();
        const data = await client.get<{ presets: Preset[] }>('/presets');
        const presets = data.presets ?? [];

        if (presets.length === 0) {
          return ok('No presets found.');
        }

        const rows = presets.map((p) => [
          p.slug,
          p.name,
          p.type ?? '',
        ]);

        return ok(
          `## Presets (${presets.length})\n\n${table(['Slug', 'Name', 'Type'], rows)}`
        );
      }),
  },

  {
    name: 'get_preset',
    description: 'Get details of a specific preset by its slug',
    inputSchema: {
      type: 'object',
      properties: {
        slug: {
          type: 'string',
          description: 'Preset slug identifier',
        },
      },
      required: ['slug'],
    },
    handler: (args) =>
      handleToolCall(async () => {
        const client = getClient();
        const slug = requireParam<string>(args, 'slug');
        const data = await client.get<{ presets: Preset[] }>('/presets');
        const presets = data.presets ?? [];

        const preset = presets.find((p) => p.slug === slug);
        if (!preset) {
          throw new Error(`Preset with slug '${slug}' not found.`);
        }

        return json('Preset Details', preset);
      }),
  },
];
