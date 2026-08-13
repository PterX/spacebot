import { useState } from "react";
import { useQuery } from "@tanstack/react-query";
import { FontAwesomeIcon } from "@fortawesome/react-fontawesome";
import {
  faClock,
  faCodeBranch,
  faExternalLinkAlt,
  faRobot,
  faFolderTree,
  faGraduationCap,
  faLock,
  faLockOpen,
  faLayerGroup,
} from "@fortawesome/free-solid-svg-icons";
import { Badge, Popover, SelectPill, OptionList, OptionListItem } from "@spacedrive/primitives";
import { api, type TaskItem } from "@/api/client";

export interface TaskEnrichment {
  at: string;
  detail: string;
}

export function taskEnrichments(runs: Awaited<ReturnType<typeof api.autonomyRuns>>["runs"]) {
  const enrichments = new Map<number, TaskEnrichment>();
  for (const run of runs) {
    for (const action of run.actions) {
      if (action.kind !== "enriched" || action.task_number === null) continue;
      const at = run.finished_at ?? run.started_at;
      const current = enrichments.get(action.task_number);
      if (!current || at > current.at) {
        enrichments.set(action.task_number, {at, detail: action.detail});
      }
    }
  }
  return enrichments;
}

function formatRelativeTime(iso: string) {
  const seconds = Math.max(0, Math.floor((Date.now() - new Date(iso).getTime()) / 1000));
  if (seconds < 60) return "just now";
  if (seconds < 3600) return `${Math.floor(seconds / 60)}m ago`;
  if (seconds < 86_400) return `${Math.floor(seconds / 3600)}h ago`;
  return `${Math.floor(seconds / 86_400)}d ago`;
}

export function TaskMetadataBadges({
  task,
  enrichment,
}: {
  task: TaskItem;
  enrichment?: TaskEnrichment;
}) {
  return (
    <div className="flex min-w-0 flex-wrap items-center gap-1">
      {enrichment && (
        <Badge
          variant="info"
          size="sm"
          title={`Researched ${formatRelativeTime(enrichment.at)}: ${enrichment.detail}`}
        >
          <FontAwesomeIcon icon={faClock} className="text-[9px]" />
          <span>Researched {formatRelativeTime(enrichment.at)}</span>
        </Badge>
      )}
      {task.depends_on.map((edge) => (
        <Badge
          key={`${edge.kind}-${edge.depends_on_task_number}`}
          variant={edge.satisfied ? "success" : "warning"}
          size="sm"
          title={`${edge.depends_on_title} - ${edge.depends_on_status}`}
        >
          <FontAwesomeIcon
            icon={edge.kind === "stack" ? faLayerGroup : edge.satisfied ? faLockOpen : faLock}
            className="text-[9px]"
          />
          <span>{edge.kind === "stack" ? "Stacks on" : "Blocked by"} SPC-{edge.depends_on_task_number}</span>
        </Badge>
      ))}
    </div>
  );
}

export function taskListTitle(task: TaskItem, enrichment?: TaskEnrichment) {
  const activity = [
    enrichment && "researched",
    ...task.depends_on.map((edge) =>
      edge.kind === "stack" ? `stacks on SPC-${edge.depends_on_task_number}` : `after SPC-${edge.depends_on_task_number}`,
    ),
  ].filter(Boolean);
  return activity.length === 0 ? task.title : `${task.title} · ${activity.join(" · ")}`;
}

// ---------------------------------------------------------------------------
// GitHub metadata helpers
// ---------------------------------------------------------------------------

interface GithubReference {
  kind: "issue" | "pr";
  label: string;
  url: string | null;
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

function toSafeExternalUrl(value: unknown): string | null {
  if (typeof value !== "string") return null;
  try {
    const parsed = new URL(value);
    if (parsed.protocol === "https:" || parsed.protocol === "http:") {
      return parsed.toString();
    }
    return null;
  } catch {
    return null;
  }
}

function readGithubReference(
  value: unknown,
  kind: GithubReference["kind"],
): GithubReference | null {
  if (!isRecord(value)) {
    return null;
  }

  const number = typeof value.number === "number" ? value.number : null;
  const repo = typeof value.repo === "string" ? value.repo : null;
  const url = toSafeExternalUrl(value.url);

  if (number === null && url === null && repo === null) {
    return null;
  }

  const noun = kind === "issue" ? "Issue" : "PR";
  const label = number !== null ? `${noun} #${number}` : repo ? `${noun} ${repo}` : noun;

  return { kind, label, url };
}

export function getGithubReferences(metadata: Record<string, unknown>): GithubReference[] {
  return [
    readGithubReference(metadata.github_issue, "issue"),
    readGithubReference(metadata.github_pr, "pr"),
  ].filter((reference): reference is GithubReference => reference !== null);
}

export function GithubMetadataBadges({
  metadata,
  references: precomputed,
  compact = false,
}: {
  metadata?: Record<string, unknown>;
  references?: GithubReference[];
  compact?: boolean;
}) {
  const references = precomputed ?? (metadata ? getGithubReferences(metadata) : []);
  if (references.length === 0) {
    return null;
  }

  return (
    <div className="flex flex-wrap items-center gap-1.5">
      {references.map((reference) => {
        const content = (
          <>
            <FontAwesomeIcon icon={faCodeBranch} className="text-[10px]" />
            <span>{reference.label}</span>
            {reference.url && (
              <FontAwesomeIcon icon={faExternalLinkAlt} className="text-[9px]" />
            )}
          </>
        );

        const className = compact
          ? "cursor-pointer hover:border-blue-400/50 hover:text-blue-300"
          : "cursor-pointer hover:border-blue-400/50 hover:bg-blue-500/20 hover:text-blue-300";

        if (reference.url) {
          return (
            <a
              key={`${reference.kind}-${reference.label}`}
              href={reference.url}
              target="_blank"
              rel="noopener noreferrer"
              className="inline-flex"
              onClick={(event) => event.stopPropagation()}
            >
              <Badge variant="info" size="sm" className={className}>
                {content}
              </Badge>
            </a>
          );
        }

        return (
          <Badge
            key={`${reference.kind}-${reference.label}`}
            variant="info"
            size="sm"
          >
            {content}
          </Badge>
        );
      })}
    </div>
  );
}

export function GithubSection({
  metadata,
}: {
  metadata: Record<string, unknown>;
}) {
  const references = getGithubReferences(metadata);
  if (references.length === 0) return null;

  return (
    <div className="border-t border-app-line/40 px-4 py-3">
      <h3 className="mb-2 text-xs font-medium uppercase tracking-wide text-ink-dull">
        GitHub Links
      </h3>
      <GithubMetadataBadges references={references} />
    </div>
  );
}

// ---------------------------------------------------------------------------
// Execution plan — where and how a task's work runs
// ---------------------------------------------------------------------------

const WORKTREE_MODE_LABELS: Record<string, string> = {
  root: "project root",
  existing: "existing worktree",
  create: "new worktree",
};

function taskHasExecutionPlan(task: TaskItem): boolean {
  return Boolean(
    task.worker_type ||
      task.project_id ||
      task.worktree_mode ||
      task.worktree_id ||
      (task.required_skills?.length ?? 0) > 0 ||
      (task.depends_on?.length ?? 0) > 0,
  );
}

export function ExecutionPlanSection({ task }: { task: TaskItem }) {
  const hasPlan = taskHasExecutionPlan(task);

  const { data: projectsData } = useQuery({
    queryKey: ["projects"],
    queryFn: () => api.listProjects("active"),
    staleTime: 30_000,
    enabled: hasPlan && Boolean(task.project_id),
  });

  // Repo and worktree names live on the project detail; fetched only when
  // the plan references one.
  const needsDetail = Boolean(task.project_id && (task.repo_id || task.worktree_id));
  const { data: projectDetail } = useQuery({
    queryKey: ["project", task.project_id],
    queryFn: () => api.getProject(task.project_id as string),
    staleTime: 30_000,
    enabled: needsDetail,
  });

  if (!hasPlan) return null;

  const project = projectsData?.projects.find((p) => p.id === task.project_id);
  const repo = projectDetail?.repos.find((r) => r.id === task.repo_id);
  const worktree = projectDetail?.worktrees.find((w) => w.id === task.worktree_id);

  const worktreeLabel = (() => {
    if (task.worktree_mode === "existing") {
      return worktree ? `worktree: ${worktree.name}` : "existing worktree";
    }
    if (task.worktree_mode) {
      return WORKTREE_MODE_LABELS[task.worktree_mode] ?? task.worktree_mode;
    }
    // A bound worktree without an explicit mode still names where work runs.
    return worktree ? `worktree: ${worktree.name}` : null;
  })();

  return (
    <div className="border-b border-app-line/40 px-4 py-3">
      <h3 className="mb-2 text-xs font-medium uppercase tracking-wide text-ink-dull">
        Execution Plan
      </h3>
      <div className="flex flex-wrap items-center gap-1.5">
        {task.worker_type && (
          <Badge
            variant={task.worker_type === "opencode" ? "accent" : "default"}
            size="sm"
          >
            <FontAwesomeIcon icon={faRobot} className="text-[10px]" />
            <span>{task.worker_type}</span>
          </Badge>
        )}
        {task.project_id && (
          <Badge variant="default" size="sm">
            <FontAwesomeIcon icon={faFolderTree} className="text-[10px]" />
            <span>{project?.name ?? "project"}</span>
            {repo && project && repo.name !== project.name && (
              <span className="text-ink-faint">/ {repo.name}</span>
            )}
          </Badge>
        )}
        {worktreeLabel && (
          <Badge variant="default" size="sm">
            <FontAwesomeIcon icon={faCodeBranch} className="text-[10px]" />
            <span>{worktreeLabel}</span>
          </Badge>
        )}
      </div>
      {(task.depends_on?.length ?? 0) > 0 && (
        <div className="mt-2">
          <div className="mb-1 text-[10px] uppercase tracking-wide text-ink-faint">
            Dependencies
          </div>
          <div className="flex flex-wrap items-center gap-1.5">
            {task.depends_on.map((edge) => (
              <Badge
                key={edge.depends_on_task_number}
                variant={edge.satisfied ? "success" : "warning"}
                size="sm"
                title={`${edge.depends_on_title} — ${edge.depends_on_status}`}
              >
                <FontAwesomeIcon
                  icon={
                    edge.kind === "stack"
                      ? faLayerGroup
                      : edge.satisfied
                        ? faLockOpen
                        : faLock
                  }
                  className="text-[10px]"
                />
                <span>
                  {edge.kind === "stack" ? "stacks on" : "after"} SPC-
                  {edge.depends_on_task_number}
                </span>
              </Badge>
            ))}
          </div>
        </div>
      )}
      {(task.required_skills?.length ?? 0) > 0 && (
        <div className="mt-2">
          <div className="mb-1 text-[10px] uppercase tracking-wide text-ink-faint">
            Required skills
          </div>
          <div className="flex flex-wrap items-center gap-1.5">
            {task.required_skills.map((skill) => (
              <Badge key={skill} variant="info" size="sm">
                <FontAwesomeIcon icon={faGraduationCap} className="text-[10px]" />
                <span>{skill}</span>
              </Badge>
            ))}
          </div>
        </div>
      )}
    </div>
  );
}

// ---------------------------------------------------------------------------
// InlineSelect — generic popover select pill
// ---------------------------------------------------------------------------

export function InlineSelect({
  value,
  options,
  onChange,
}: {
  value: string;
  options: { value: string; label: string }[];
  onChange: (value: string) => void;
}) {
  const [open, setOpen] = useState(false);
  const selectedLabel = options.find((o) => o.value === value)?.label ?? value;

  return (
    <Popover.Root open={open} onOpenChange={setOpen}>
      <Popover.Trigger asChild>
        <SelectPill size="sm" className="w-full">{selectedLabel}</SelectPill>
      </Popover.Trigger>
      <Popover.Content align="start" sideOffset={4} className="min-w-[160px] p-1.5">
        <OptionList>
          {options.map((opt) => (
            <OptionListItem
              key={opt.value}
              selected={opt.value === value}
              size="sm"
              onClick={() => {
                onChange(opt.value);
                setOpen(false);
              }}
            >
              {opt.label}
            </OptionListItem>
          ))}
        </OptionList>
      </Popover.Content>
    </Popover.Root>
  );
}
