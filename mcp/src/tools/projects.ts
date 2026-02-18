import { getClient } from '../api/index.js';
import { ok, json, table, formatDate, handleToolCall, requireParam, optionalParam } from './_helpers.js';
import type { ToolDefinition } from '../types/index.js';

interface ProjectResponse {
  id: number;
  slug: string;
  name: string;
  repo_name: string | null;
  repo_owner: string | null;
  directory: string;
  main_branch: string;
  preset: string | null;
  created_at: number;
  updated_at: number;
  last_deployment: number | null;
  git_provider_connection_id: number | null;
  attack_mode: boolean;
  enable_preview_environments: boolean;
  source_type: string;
}

interface PaginatedProjectList {
  projects: ProjectResponse[];
  total: number;
  page: number;
  per_page: number;
}

interface ProjectStatisticsResponse {
  total_count: number;
}

function formatProjectDetails(p: ProjectResponse): string {
  const lines = [
    `## Project: ${p.name}`,
    '',
    `| Field | Value |`,
    `| --- | --- |`,
    `| ID | ${p.id} |`,
    `| Slug | ${p.slug} |`,
    `| Name | ${p.name} |`,
    `| Repository | ${p.repo_owner ?? '—'}/${p.repo_name ?? '—'} |`,
    `| Main Branch | ${p.main_branch} |`,
    `| Directory | ${p.directory} |`,
    `| Preset | ${p.preset ?? '—'} |`,
    `| Source Type | ${p.source_type} |`,
    `| Attack Mode | ${p.attack_mode} |`,
    `| Preview Environments | ${p.enable_preview_environments} |`,
    `| Git Provider Connection | ${p.git_provider_connection_id ?? '—'} |`,
    `| Created | ${formatDate(p.created_at)} |`,
    `| Updated | ${formatDate(p.updated_at)} |`,
    `| Last Deployment | ${p.last_deployment ? formatDate(p.last_deployment) : '—'} |`,
  ];
  return lines.join('\n');
}

export const tools: ToolDefinition[] = [
  // ── list_projects ──────────────────────────────────────────────
  {
    name: 'list_projects',
    description: 'List all projects with pagination',
    inputSchema: {
      type: 'object',
      properties: {
        page: { type: 'number', description: 'Page number (default: 1)' },
        per_page: { type: 'number', description: 'Items per page (default: 20, max: 100)' },
      },
    },
    handler: (args) =>
      handleToolCall(async () => {
        const page = optionalParam<number>(args, 'page');
        const per_page = optionalParam<number>(args, 'per_page');

        const data = await getClient().get<PaginatedProjectList>('/projects', { page, per_page });

        if (!data.projects.length) {
          return ok('No projects found.');
        }

        const header = `**Projects** (${data.total} total — page ${data.page}/${Math.ceil(data.total / data.per_page)})\n\n`;

        const t = table(
          ['ID', 'Name', 'Slug', 'Repository', 'Branch', 'Preset', 'Source', 'Created'],
          data.projects.map((p) => [
            String(p.id),
            p.name,
            p.slug,
            p.repo_owner && p.repo_name ? `${p.repo_owner}/${p.repo_name}` : '—',
            p.main_branch,
            p.preset ?? '—',
            p.source_type,
            formatDate(p.created_at),
          ]),
        );

        return ok(header + t);
      }),
  },

  // ── get_project ────────────────────────────────────────────────
  {
    name: 'get_project',
    description: 'Get project details by ID',
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
        const project = await getClient().get<ProjectResponse>(`/projects/${id}`);
        return ok(formatProjectDetails(project));
      }),
  },

  // ── get_project_by_slug ────────────────────────────────────────
  {
    name: 'get_project_by_slug',
    description: 'Get project details by slug',
    inputSchema: {
      type: 'object',
      properties: {
        slug: { type: 'string', description: 'Project slug' },
      },
      required: ['slug'],
    },
    handler: (args) =>
      handleToolCall(async () => {
        const slug = requireParam<string>(args, 'slug');
        const project = await getClient().get<ProjectResponse>(`/projects/by-slug/${encodeURIComponent(slug)}`);
        return ok(formatProjectDetails(project));
      }),
  },

  // ── create_project ─────────────────────────────────────────────
  {
    name: 'create_project',
    description: 'Create a new project',
    inputSchema: {
      type: 'object',
      properties: {
        name: { type: 'string', description: 'Project name' },
        main_branch: { type: 'string', description: 'Main branch (default: main)' },
        directory: { type: 'string', description: 'Project directory (default: /)' },
        preset: { type: 'string', description: 'Build preset (e.g. nixpacks, dockerfile)' },
        repo_name: { type: 'string', description: 'Repository name' },
        repo_owner: { type: 'string', description: 'Repository owner' },
        git_url: { type: 'string', description: 'Git URL for the repository' },
        git_provider_connection_id: { type: 'number', description: 'Git provider connection ID' },
        automatic_deploy: { type: 'boolean', description: 'Enable automatic deploys on push' },
        source_type: { type: 'string', description: 'Source type: git, docker_image, or static_files', enum: ['git', 'docker_image', 'static_files'] },
      },
      required: ['name'],
    },
    handler: (args) =>
      handleToolCall(async () => {
        const name = requireParam<string>(args, 'name');

        const body: Record<string, unknown> = {
          name,
          main_branch: optionalParam<string>(args, 'main_branch') ?? 'main',
          directory: optionalParam<string>(args, 'directory') ?? '/',
          preset: optionalParam<string>(args, 'preset') ?? 'nixpacks',
          storage_service_ids: [],
        };

        const optionalFields = [
          'repo_name', 'repo_owner', 'git_url', 'git_provider_connection_id',
          'automatic_deploy', 'source_type',
        ];
        for (const field of optionalFields) {
          const val = optionalParam(args, field);
          if (val !== undefined) body[field] = val;
        }

        const project = await getClient().post<ProjectResponse>('/projects', body);
        return ok(`Project created successfully.\n\n${formatProjectDetails(project)}`);
      }),
  },

  // ── update_project ─────────────────────────────────────────────
  {
    name: 'update_project',
    description: 'Update a project by ID',
    inputSchema: {
      type: 'object',
      properties: {
        id: { type: 'number', description: 'Project ID' },
        name: { type: 'string', description: 'New project name' },
        main_branch: { type: 'string', description: 'New main branch' },
        directory: { type: 'string', description: 'New project directory' },
        preset: { type: 'string', description: 'New build preset' },
      },
      required: ['id'],
    },
    handler: (args) =>
      handleToolCall(async () => {
        const id = requireParam<number>(args, 'id');
        const client = getClient();

        // GET current project to merge with updates (API requires all fields)
        const current = await client.get<ProjectResponse>(`/projects/${id}`);

        const body: Record<string, unknown> = {
          name: current.name,
          main_branch: current.main_branch,
          directory: current.directory,
          preset: current.preset ?? 'custom',
          storage_service_ids: [],
        };
        for (const field of ['name', 'main_branch', 'directory', 'preset']) {
          const val = optionalParam(args, field);
          if (val !== undefined) body[field] = val;
        }

        const project = await client.put<ProjectResponse>(`/projects/${id}`, body);
        return ok(`Project updated successfully.\n\n${formatProjectDetails(project)}`);
      }),
  },

  // ── delete_project ─────────────────────────────────────────────
  {
    name: 'delete_project',
    description: 'Delete a project by ID',
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
        await getClient().delete(`/projects/${id}`);
        return ok(`Project ${id} deleted successfully.`);
      }),
  },

  // ── update_project_settings ────────────────────────────────────
  {
    name: 'update_project_settings',
    description: 'Update project settings (slug, attack mode, preview environments)',
    inputSchema: {
      type: 'object',
      properties: {
        project_id: { type: 'number', description: 'Project ID' },
        slug: { type: 'string', description: 'New project slug' },
        attack_mode: { type: 'boolean', description: 'Enable/disable attack mode (CAPTCHA protection)' },
        enable_preview_environments: { type: 'boolean', description: 'Enable automatic preview environments per branch' },
      },
      required: ['project_id'],
    },
    handler: (args) =>
      handleToolCall(async () => {
        const projectId = requireParam<number>(args, 'project_id');

        const body: Record<string, unknown> = {};
        for (const field of ['slug', 'attack_mode', 'enable_preview_environments']) {
          const val = optionalParam(args, field);
          if (val !== undefined) body[field] = val;
        }

        const settings = await getClient().post(`/projects/${projectId}/settings`, body);
        return json('Project Settings Updated', settings);
      }),
  },

  // ── update_git_settings ────────────────────────────────────────
  {
    name: 'update_git_settings',
    description: 'Update git settings for a project',
    inputSchema: {
      type: 'object',
      properties: {
        project_id: { type: 'number', description: 'Project ID' },
        repo_owner: { type: 'string', description: 'Repository owner' },
        repo_name: { type: 'string', description: 'Repository name' },
        main_branch: { type: 'string', description: 'Main branch' },
        directory: { type: 'string', description: 'Project directory' },
        preset: { type: 'string', description: 'Build preset' },
        git_provider_connection_id: { type: 'number', description: 'Git provider connection ID' },
      },
      required: ['project_id', 'repo_owner', 'repo_name', 'main_branch', 'directory'],
    },
    handler: (args) =>
      handleToolCall(async () => {
        const projectId = requireParam<number>(args, 'project_id');

        const body: Record<string, unknown> = {
          repo_owner: requireParam<string>(args, 'repo_owner'),
          repo_name: requireParam<string>(args, 'repo_name'),
          main_branch: requireParam<string>(args, 'main_branch'),
          directory: requireParam<string>(args, 'directory'),
        };

        for (const field of ['preset', 'git_provider_connection_id']) {
          const val = optionalParam(args, field);
          if (val !== undefined) body[field] = val;
        }

        const settings = await getClient().post(`/projects/${projectId}/git`, body);
        return json('Git Settings Updated', settings);
      }),
  },

  // ── get_project_statistics ─────────────────────────────────────
  {
    name: 'get_project_statistics',
    description: 'Get overall project statistics',
    inputSchema: {
      type: 'object',
      properties: {},
    },
    handler: () =>
      handleToolCall(async () => {
        const stats = await getClient().get<ProjectStatisticsResponse>('/projects/statistics');
        return ok(`## Project Statistics\n\n- **Total Projects**: ${stats.total_count}`);
      }),
  },
];
