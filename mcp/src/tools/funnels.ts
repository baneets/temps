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

interface FunnelStep {
  event_name: string;
  filters?: Record<string, unknown>;
}

interface Funnel {
  id: number;
  name: string;
  steps: FunnelStep[];
  project_id: number;
  created_at?: string;
  updated_at?: string;
}

interface FunnelMetrics {
  funnel_name: string;
  total_entries: number;
  overall_conversion_rate: number;
  step_conversions: Array<Record<string, unknown>>;
}

export const tools: ToolDefinition[] = [
  {
    name: 'list_funnels',
    description: 'List all funnels for a project',
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
        const client = getClient();
        const projectId = requireParam<number>(args, 'project_id');
        const funnels = await client.get<Funnel[]>(
          `/projects/${projectId}/funnels`
        );

        if (!funnels || funnels.length === 0) {
          return ok('No funnels found for this project.');
        }

        const rows = funnels.map((f) => [
          String(f.id),
          f.name ?? '',
        ]);

        return ok(
          `## Funnels for Project ${projectId}\n\n${table(['ID', 'Name'], rows)}`
        );
      }),
  },

  {
    name: 'create_funnel',
    description:
      'Create a new funnel with a sequence of steps for a project',
    inputSchema: {
      type: 'object',
      properties: {
        project_id: {
          type: 'number',
          description: 'Project ID',
        },
        name: {
          type: 'string',
          description: 'Funnel name',
        },
        steps: {
          type: 'array',
          description: 'Ordered list of funnel steps',
          items: {
            type: 'object',
            properties: {
              event_name: {
                type: 'string',
                description: 'Event name for this step',
              },
              filters: {
                type: 'object',
                description: 'Optional filters for this step',
              },
            },
            required: ['event_name'],
          },
        },
      },
      required: ['project_id', 'name', 'steps'],
    },
    handler: (args) =>
      handleToolCall(async () => {
        const client = getClient();
        const projectId = requireParam<number>(args, 'project_id');
        const name = requireParam<string>(args, 'name');
        const steps = requireParam<FunnelStep[]>(args, 'steps');

        const result = await client.post<{ funnel_id: number }>(
          `/projects/${projectId}/funnels`,
          { name, steps }
        );
        return json('Funnel Created', result);
      }),
  },

  {
    name: 'update_funnel',
    description: 'Update an existing funnel name and/or steps',
    inputSchema: {
      type: 'object',
      properties: {
        project_id: {
          type: 'number',
          description: 'Project ID',
        },
        funnel_id: {
          type: 'number',
          description: 'Funnel ID',
        },
        name: {
          type: 'string',
          description: 'Updated funnel name',
        },
        steps: {
          type: 'array',
          description: 'Updated ordered list of funnel steps',
          items: {
            type: 'object',
            properties: {
              event_name: {
                type: 'string',
                description: 'Event name for this step',
              },
              filters: {
                type: 'object',
                description: 'Optional filters for this step',
              },
            },
            required: ['event_name'],
          },
        },
      },
      required: ['project_id', 'funnel_id'],
    },
    handler: (args) =>
      handleToolCall(async () => {
        const client = getClient();
        const projectId = requireParam<number>(args, 'project_id');
        const funnelId = requireParam<number>(args, 'funnel_id');
        const name = optionalParam<string>(args, 'name');
        const steps = optionalParam<FunnelStep[]>(args, 'steps');

        // API requires both name and steps; fetch current from list to merge
        let currentName = name ?? 'Funnel';
        let currentSteps = steps ?? [{ event_name: 'page_view' }];
        if (!name || !steps) {
          const funnels = await client.get<Funnel[]>(
            `/projects/${projectId}/funnels`
          );
          const current = funnels.find((f) => f.id === funnelId);
          if (current) {
            currentName = name ?? current.name;
            currentSteps = steps ?? current.steps ?? currentSteps;
          }
        }
        const body: Record<string, unknown> = {
          name: currentName,
          steps: currentSteps,
        };

        const funnel = await client.put<Funnel>(
          `/projects/${projectId}/funnels/${funnelId}`,
          body
        );
        return json('Funnel Updated', funnel);
      }),
  },

  {
    name: 'delete_funnel',
    description: 'Delete a funnel from a project',
    inputSchema: {
      type: 'object',
      properties: {
        project_id: {
          type: 'number',
          description: 'Project ID',
        },
        funnel_id: {
          type: 'number',
          description: 'Funnel ID',
        },
      },
      required: ['project_id', 'funnel_id'],
    },
    handler: (args) =>
      handleToolCall(async () => {
        const client = getClient();
        const projectId = requireParam<number>(args, 'project_id');
        const funnelId = requireParam<number>(args, 'funnel_id');
        await client.delete(
          `/projects/${projectId}/funnels/${funnelId}`
        );
        return ok(
          `Funnel ${funnelId} deleted from project ${projectId}.`
        );
      }),
  },

  {
    name: 'get_funnel_metrics',
    description:
      'Get conversion metrics for a specific funnel including step-by-step conversion rates',
    inputSchema: {
      type: 'object',
      properties: {
        project_id: {
          type: 'number',
          description: 'Project ID',
        },
        funnel_id: {
          type: 'number',
          description: 'Funnel ID',
        },
      },
      required: ['project_id', 'funnel_id'],
    },
    handler: (args) =>
      handleToolCall(async () => {
        const client = getClient();
        const projectId = requireParam<number>(args, 'project_id');
        const funnelId = requireParam<number>(args, 'funnel_id');

        const metrics = await client.get<FunnelMetrics>(
          `/projects/${projectId}/funnels/${funnelId}/metrics`
        );

        let text = `## Funnel Metrics: ${metrics.funnel_name}\n\n`;
        text += `- **Total Entries:** ${metrics.total_entries}\n`;
        text += `- **Overall Conversion Rate:** ${metrics.overall_conversion_rate}%\n\n`;
        text += `### Step Conversions\n\n`;
        text += `\`\`\`json\n${JSON.stringify(metrics.step_conversions, null, 2)}\n\`\`\``;

        return ok(text);
      }),
  },

  {
    name: 'preview_funnel_metrics',
    description:
      'Preview conversion metrics for a funnel definition without saving it',
    inputSchema: {
      type: 'object',
      properties: {
        project_id: {
          type: 'number',
          description: 'Project ID',
        },
        name: {
          type: 'string',
          description: 'Funnel name',
        },
        steps: {
          type: 'array',
          description: 'Ordered list of funnel steps to preview',
          items: {
            type: 'object',
            properties: {
              event_name: {
                type: 'string',
                description: 'Event name for this step',
              },
              filters: {
                type: 'object',
                description: 'Optional filters for this step',
              },
            },
            required: ['event_name'],
          },
        },
      },
      required: ['project_id', 'name', 'steps'],
    },
    handler: (args) =>
      handleToolCall(async () => {
        const client = getClient();
        const projectId = requireParam<number>(args, 'project_id');
        const name = requireParam<string>(args, 'name');
        const steps = requireParam<FunnelStep[]>(args, 'steps');

        const metrics = await client.post<FunnelMetrics>(
          `/projects/${projectId}/funnels/preview`,
          { name, steps }
        );

        let text = `## Funnel Preview: ${metrics.funnel_name}\n\n`;
        text += `- **Total Entries:** ${metrics.total_entries}\n`;
        text += `- **Overall Conversion Rate:** ${metrics.overall_conversion_rate}%\n\n`;
        text += `### Step Conversions\n\n`;
        text += `\`\`\`json\n${JSON.stringify(metrics.step_conversions, null, 2)}\n\`\`\``;

        return ok(text);
      }),
  },
];
