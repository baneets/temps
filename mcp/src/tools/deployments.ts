import { getClient } from '../api/index.js';
import { ok, json, table, formatDate, handleToolCall, requireParam, optionalParam } from './_helpers.js';
import type { ToolDefinition } from '../types/index.js';

interface DeploymentEnvironment {
  id: number;
  name: string;
  slug: string;
  domains: string[];
}

interface DeploymentResponse {
  id: number;
  project_id: number;
  environment_id: number;
  environment: DeploymentEnvironment;
  status: string;
  url: string;
  commit_hash: string | null;
  commit_message: string | null;
  branch: string | null;
  tag: string | null;
  created_at: number;
  started_at: number | null;
  finished_at: number | null;
  commit_author: string | null;
  commit_date: number | null;
  is_current: boolean;
  cancelled_reason: string | null;
}

interface DeploymentListResponse {
  deployments: DeploymentResponse[];
  total: number;
  page: number;
  per_page: number;
}

interface DeploymentJobResponse {
  id: number;
  deployment_id: number;
  job_id: string;
  job_type: string;
  name: string;
  description: string | null;
  status: string;
  created_at: number;
  updated_at: number;
  started_at: number | null;
  finished_at: number | null;
  log_id: string;
  error_message: string | null;
  execution_order: number | null;
}

interface DeploymentJobsResponse {
  jobs: DeploymentJobResponse[];
  total: number;
}

interface DeploymentStateResponse {
  id: number;
  state: string;
  message: string;
}

interface TriggerPipelineResponse {
  message: string;
  project_id: number;
  environment_id: number;
  branch: string | null;
  tag: string | null;
  commit: string | null;
}

function formatDeploymentDetails(d: DeploymentResponse): string {
  const lines = [
    `## Deployment #${d.id}`,
    '',
    `| Field | Value |`,
    `| --- | --- |`,
    `| ID | ${d.id} |`,
    `| Project ID | ${d.project_id} |`,
    `| Status | ${d.status} |`,
    `| URL | ${d.url} |`,
    `| Environment | ${d.environment.name} (${d.environment.slug}) |`,
    `| Branch | ${d.branch ?? '—'} |`,
    `| Tag | ${d.tag ?? '—'} |`,
    `| Commit | ${d.commit_hash ?? '—'} |`,
    `| Commit Message | ${d.commit_message ?? '—'} |`,
    `| Commit Author | ${d.commit_author ?? '—'} |`,
    `| Is Current | ${d.is_current} |`,
    `| Created | ${formatDate(d.created_at)} |`,
    `| Started | ${d.started_at ? formatDate(d.started_at) : '—'} |`,
    `| Finished | ${d.finished_at ? formatDate(d.finished_at) : '—'} |`,
  ];

  if (d.cancelled_reason) {
    lines.push(`| Cancelled Reason | ${d.cancelled_reason} |`);
  }

  if (d.environment.domains.length) {
    lines.push(`| Domains | ${d.environment.domains.join(', ')} |`);
  }

  return lines.join('\n');
}

export const tools: ToolDefinition[] = [
  // ── list_deployments ───────────────────────────────────────────
  {
    name: 'list_deployments',
    description: 'List deployments for a project',
    inputSchema: {
      type: 'object',
      properties: {
        project_id: { type: 'number', description: 'Project ID' },
        page: { type: 'number', description: 'Page number (default: 1)' },
        per_page: { type: 'number', description: 'Items per page (default: 20, max: 100)' },
        environment_id: { type: 'number', description: 'Filter by environment ID' },
      },
      required: ['project_id'],
    },
    handler: (args) =>
      handleToolCall(async () => {
        const projectId = requireParam<number>(args, 'project_id');
        const page = optionalParam<number>(args, 'page');
        const per_page = optionalParam<number>(args, 'per_page');
        const environment_id = optionalParam<number>(args, 'environment_id');

        const data = await getClient().get<DeploymentListResponse>(
          `/projects/${projectId}/deployments`,
          { page, per_page, environment_id },
        );

        if (!data.deployments.length) {
          return ok('No deployments found.');
        }

        const header = `**Deployments** (${data.total} total — page ${data.page}/${Math.ceil(data.total / data.per_page)})\n\n`;

        const t = table(
          ['ID', 'Status', 'Branch', 'Commit', 'Environment', 'Current', 'Created'],
          data.deployments.map((d) => [
            String(d.id),
            d.status,
            d.branch ?? '—',
            d.commit_hash ? d.commit_hash.substring(0, 8) : '—',
            d.environment.name,
            d.is_current ? 'Yes' : 'No',
            formatDate(d.created_at),
          ]),
        );

        return ok(header + t);
      }),
  },

  // ── get_deployment ─────────────────────────────────────────────
  {
    name: 'get_deployment',
    description: 'Get deployment details',
    inputSchema: {
      type: 'object',
      properties: {
        project_id: { type: 'number', description: 'Project ID' },
        deployment_id: { type: 'number', description: 'Deployment ID' },
      },
      required: ['project_id', 'deployment_id'],
    },
    handler: (args) =>
      handleToolCall(async () => {
        const projectId = requireParam<number>(args, 'project_id');
        const deploymentId = requireParam<number>(args, 'deployment_id');

        const deployment = await getClient().get<DeploymentResponse>(
          `/projects/${projectId}/deployments/${deploymentId}`,
        );
        return ok(formatDeploymentDetails(deployment));
      }),
  },

  // ── trigger_deployment ─────────────────────────────────────────
  {
    name: 'trigger_deployment',
    description: 'Trigger a new deployment pipeline for a project',
    inputSchema: {
      type: 'object',
      properties: {
        id: { type: 'number', description: 'Project ID' },
        branch: { type: 'string', description: 'Branch to deploy' },
        commit: { type: 'string', description: 'Specific commit hash to deploy' },
        environment_id: { type: 'number', description: 'Target environment ID' },
      },
      required: ['id', 'branch', 'environment_id'],
    },
    handler: (args) =>
      handleToolCall(async () => {
        const id = requireParam<number>(args, 'id');
        const branch = requireParam<string>(args, 'branch');
        const environment_id = requireParam<number>(args, 'environment_id');
        const commit = optionalParam<string>(args, 'commit');

        const body: Record<string, unknown> = { branch, environment_id };
        if (commit !== undefined) body.commit = commit;

        const result = await getClient().post<TriggerPipelineResponse>(
          `/projects/${id}/trigger-pipeline`,
          body,
        );

        return ok(
          `## Pipeline Triggered\n\n` +
          `- **Message**: ${result.message}\n` +
          `- **Project ID**: ${result.project_id}\n` +
          `- **Environment ID**: ${result.environment_id}\n` +
          `- **Branch**: ${result.branch ?? '—'}\n` +
          `- **Commit**: ${result.commit ?? '—'}`,
        );
      }),
  },

  // ── cancel_deployment ──────────────────────────────────────────
  {
    name: 'cancel_deployment',
    description: 'Cancel a running deployment',
    inputSchema: {
      type: 'object',
      properties: {
        project_id: { type: 'number', description: 'Project ID' },
        deployment_id: { type: 'number', description: 'Deployment ID' },
      },
      required: ['project_id', 'deployment_id'],
    },
    handler: (args) =>
      handleToolCall(async () => {
        const projectId = requireParam<number>(args, 'project_id');
        const deploymentId = requireParam<number>(args, 'deployment_id');

        const result = await getClient().post<DeploymentStateResponse>(
          `/projects/${projectId}/deployments/${deploymentId}/cancel`,
        );
        return ok(`Deployment ${result.id} cancelled: ${result.message}`);
      }),
  },

  // ── pause_deployment ───────────────────────────────────────────
  {
    name: 'pause_deployment',
    description: 'Pause a running deployment',
    inputSchema: {
      type: 'object',
      properties: {
        project_id: { type: 'number', description: 'Project ID' },
        deployment_id: { type: 'number', description: 'Deployment ID' },
      },
      required: ['project_id', 'deployment_id'],
    },
    handler: (args) =>
      handleToolCall(async () => {
        const projectId = requireParam<number>(args, 'project_id');
        const deploymentId = requireParam<number>(args, 'deployment_id');

        const result = await getClient().post<DeploymentStateResponse>(
          `/projects/${projectId}/deployments/${deploymentId}/pause`,
        );
        return ok(`Deployment ${result.id} paused: ${result.message}`);
      }),
  },

  // ── resume_deployment ──────────────────────────────────────────
  {
    name: 'resume_deployment',
    description: 'Resume a paused deployment',
    inputSchema: {
      type: 'object',
      properties: {
        project_id: { type: 'number', description: 'Project ID' },
        deployment_id: { type: 'number', description: 'Deployment ID' },
      },
      required: ['project_id', 'deployment_id'],
    },
    handler: (args) =>
      handleToolCall(async () => {
        const projectId = requireParam<number>(args, 'project_id');
        const deploymentId = requireParam<number>(args, 'deployment_id');

        const result = await getClient().post<DeploymentStateResponse>(
          `/projects/${projectId}/deployments/${deploymentId}/resume`,
        );
        return ok(`Deployment ${result.id} resumed: ${result.message}`);
      }),
  },

  // ── teardown_deployment ────────────────────────────────────────
  {
    name: 'teardown_deployment',
    description: 'Teardown a deployment (stop and remove containers)',
    inputSchema: {
      type: 'object',
      properties: {
        project_id: { type: 'number', description: 'Project ID' },
        deployment_id: { type: 'number', description: 'Deployment ID' },
      },
      required: ['project_id', 'deployment_id'],
    },
    handler: (args) =>
      handleToolCall(async () => {
        const projectId = requireParam<number>(args, 'project_id');
        const deploymentId = requireParam<number>(args, 'deployment_id');

        await getClient().delete(
          `/projects/${projectId}/deployments/${deploymentId}/teardown`,
        );
        return ok(`Deployment ${deploymentId} torn down successfully.`);
      }),
  },

  // ── rollback_deployment ────────────────────────────────────────
  {
    name: 'rollback_deployment',
    description: 'Rollback to a previous deployment',
    inputSchema: {
      type: 'object',
      properties: {
        project_id: { type: 'number', description: 'Project ID' },
        deployment_id: { type: 'number', description: 'Deployment ID to rollback to' },
      },
      required: ['project_id', 'deployment_id'],
    },
    handler: (args) =>
      handleToolCall(async () => {
        const projectId = requireParam<number>(args, 'project_id');
        const deploymentId = requireParam<number>(args, 'deployment_id');

        const deployment = await getClient().post<DeploymentResponse>(
          `/projects/${projectId}/deployments/${deploymentId}/rollback`,
        );
        return ok(`Rollback initiated.\n\n${formatDeploymentDetails(deployment)}`);
      }),
  },

  // ── get_deployment_logs ────────────────────────────────────────
  {
    name: 'get_deployment_logs',
    description: 'Get combined logs for all jobs in a deployment',
    inputSchema: {
      type: 'object',
      properties: {
        project_id: { type: 'number', description: 'Project ID' },
        deployment_id: { type: 'number', description: 'Deployment ID' },
      },
      required: ['project_id', 'deployment_id'],
    },
    handler: (args) =>
      handleToolCall(async () => {
        const projectId = requireParam<number>(args, 'project_id');
        const deploymentId = requireParam<number>(args, 'deployment_id');
        const client = getClient();

        // Fetch all jobs for this deployment
        const jobsData = await client.get<DeploymentJobsResponse>(
          `/projects/${projectId}/deployments/${deploymentId}/jobs`,
        );

        if (!jobsData.jobs.length) {
          return ok('No jobs found for this deployment.');
        }

        const sections: string[] = [];

        for (const job of jobsData.jobs) {
          const header = `### ${job.name} (${job.status})`;

          let logContent: string;
          try {
            logContent = await client.get<string>(
              `/projects/${projectId}/deployments/${deploymentId}/jobs/${encodeURIComponent(job.job_id)}/logs`,
            );
          } catch {
            logContent = job.error_message ?? '(no logs available)';
          }

          sections.push(`${header}\n\`\`\`\n${logContent}\n\`\`\``);
        }

        return ok(`## Deployment ${deploymentId} Logs\n\n${sections.join('\n\n')}`);
      }),
  },

  // ── get_last_deployment ────────────────────────────────────────
  {
    name: 'get_last_deployment',
    description: 'Get the last deployment for a project',
    inputSchema: {
      type: 'object',
      properties: {
        id: { type: 'number', description: 'Project ID' },
      },
      required: ['id'],
    },
    handler: (args) =>
      handleToolCall(async () => {
        const id = requireParam<number>(args, 'id');
        const deployment = await getClient().get<DeploymentResponse>(`/projects/${id}/last-deployment`);
        return ok(formatDeploymentDetails(deployment));
      }),
  },
];
