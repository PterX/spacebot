import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { Link } from "@tanstack/react-router";
import {
  api,
  type ChannelInfo,
  type TimelineItem,
  type TimelineBranchRun,
  type TimelineCheckpoint,
  type TimelineWorkerRun,
} from "@/api/client";
import {
  isOpenCodeWorker,
  type ChannelLiveState,
  type ActiveWorker,
  type ActiveBranch,
} from "@/hooks/useChannelLiveState";
import { useLiveContext } from "@/hooks/useLiveContext";
import { Markdown } from "@/components/Markdown";
import { PromptInspectModal } from "@/components/PromptInspectModal";
import {
  ProcessCard,
  ProcessDetail,
  branchRunStatus,
  type ProcessRunDisplay,
  type ProcessSelection,
} from "@/components/processes/ProcessRunView";
import { formatTimestamp, platformIcon, platformColor } from "@/lib/format";
import { Button } from "@spacedrive/primitives";
import { Code } from "@phosphor-icons/react";

interface ChannelDetailProps {
  agentId: string;
  channelId: string;
  channel: ChannelInfo | undefined;
  liveState: ChannelLiveState | undefined;
  onLoadMore: () => void;
}

function LiveBranchRunItem({
  item,
  live,
  channelId,
  selected,
  onSelect,
}: {
  item: TimelineBranchRun;
  live: ActiveBranch;
  channelId: string;
  selected: boolean;
  onSelect: () => void;
}) {
  return (
    <ProcessCard
      kind="branch"
      id={item.id}
      title={item.description}
      status="running"
      startedAt={item.started_at}
      toolCalls={live.toolCalls}
      currentTool={live.currentTool ?? live.lastTool}
      selected={selected}
      onSelect={onSelect}
      onCancel={() => api.cancelProcess(channelId, "branch", item.id).catch(console.warn)}
    />
  );
}

function LiveWorkerRunItem({
  item,
  live,
  channelId,
  selected,
  onSelect,
}: {
  item: TimelineWorkerRun;
  live: ActiveWorker;
  channelId: string;
  selected: boolean;
  onSelect: () => void;
}) {
  return (
    <ProcessCard
      kind="worker"
      id={item.id}
      title={item.task}
      status={live.isIdle ? "idle" : "running"}
      startedAt={item.started_at}
      toolCalls={live.toolCalls}
      currentTool={live.currentTool ?? live.status}
      processType={live.workerType}
      selected={selected}
      onSelect={onSelect}
      onCancel={() => api.cancelProcess(channelId, "worker", item.id).catch(console.warn)}
    />
  );
}

function BranchRunItem({ item, selected, onSelect }: { item: TimelineBranchRun; selected: boolean; onSelect: () => void }) {
  const status = branchRunStatus(item.conclusion);
  return (
    <ProcessCard kind="branch" id={item.id} title={item.description} status={status} startedAt={item.started_at} selected={selected} onSelect={onSelect} />
  );
}

function WorkerRunItem({
  item,
  selected,
  onSelect,
}: {
  item: TimelineWorkerRun;
  selected: boolean;
  onSelect: () => void;
}) {
  return (
    <ProcessCard kind="worker" id={item.id} title={item.task} status={item.status} startedAt={item.started_at} processType={isOpenCodeWorker({task: item.task}) ? "opencode" : "builtin"} selected={selected} onSelect={onSelect} />
  );
}

/** Range label for a checkpoint's coverage, collapsed when it's a single day. */
function coverageRange(from: string, to: string): string {
  const start = new Date(from);
  const end = new Date(to);
  const date = (value: Date) =>
    value.toLocaleDateString(undefined, { month: "short", day: "numeric" });
  const time = (value: Date) =>
    value.toLocaleTimeString(undefined, { hour: "2-digit", minute: "2-digit" });
  return start.toDateString() === end.toDateString()
    ? `${date(start)} · ${time(start)}–${time(end)}`
    : `${date(start)} ${time(start)} → ${date(end)} ${time(end)}`;
}

/**
 * A chronicle checkpoint, rendered inline where the conversation reached it.
 * Deliberately unlike the message rows on either side: this was authored by
 * neither the user nor the agent.
 */
function CheckpointItem({ item }: { item: TimelineCheckpoint }) {
  const [expanded, setExpanded] = useState(false);
  const emergency = item.kind === "emergency";

  return (
    <div className="flex gap-3 px-3 py-2">
      <span className="flex-shrink-0 pt-0.5 text-tiny text-ink-faint">
        {formatTimestamp(new Date(item.created_at).getTime())}
      </span>
      <div className="min-w-0 flex-1">
        <button
          type="button"
          onClick={() => setExpanded((value) => !value)}
          className={`w-full rounded-md border border-dashed px-3 py-2 text-left transition-colors ${
            emergency
              ? "border-status-error/30 bg-status-error/5 hover:bg-status-error/10"
              : "border-ink-faint/25 bg-app-dark-box/40 hover:bg-app-dark-box/60"
          }`}
        >
          <div className="flex min-w-0 items-baseline gap-2">
            <span
              className={`flex-shrink-0 text-tiny font-medium uppercase tracking-wide ${
                emergency ? "text-status-error/80" : "text-ink-faint"
              }`}
            >
              {item.level > 0 ? "Rollup" : "Checkpoint"} #{item.seq}
            </span>
            <span className="min-w-0 flex-1 truncate text-sm text-ink-dull">
              {item.title}
            </span>
            <span className="flex-shrink-0 text-tiny leading-5 text-ink-faint">
              {expanded ? "▾" : "▸"}
            </span>
          </div>
          <div className="mt-1 flex flex-wrap items-center gap-x-3 gap-y-1 text-tiny text-ink-faint">
            <span>{coverageRange(item.covers_from, item.covers_to)}</span>
            <span>{item.message_count} messages</span>
            {item.kind !== "interval" && <span>{item.kind}</span>}
            {item.rolled_up_into && <span>rolled up</span>}
          </div>
        </button>
        {expanded && (
          <div className="mt-1 rounded-md border border-ink-faint/10 bg-app-dark-box/30 px-3 py-2">
            <div className="text-sm text-ink-dull">
              <Markdown className="break-words">{item.summary}</Markdown>
            </div>
          </div>
        )}
      </div>
    </div>
  );
}

function TimelineEntry({
  item,
  liveWorkers,
  liveBranches,
  channelId,
  selection,
  onSelect,
}: {
  item: TimelineItem;
  liveWorkers: Record<string, ActiveWorker>;
  liveBranches: Record<string, ActiveBranch>;
  channelId: string;
  selection: ProcessSelection | null;
  onSelect: (selection: ProcessSelection) => void;
}) {
  switch (item.type) {
    case "message":
      return (
        <div
          className={`flex gap-3 rounded-md px-3 py-2 ${
            item.role === "user" ? "bg-app-dark-box/30" : ""
          }`}
        >
          <span className="flex-shrink-0 pt-0.5 text-tiny text-ink-faint">
            {formatTimestamp(new Date(item.created_at).getTime())}
          </span>
          <div className="min-w-0 flex-1">
            <span
              className={`text-sm font-medium ${
                item.role === "user"
                  ? "text-accent-faint"
                  : "text-status-success"
              }`}
            >
              {item.role === "user"
                ? (item.sender_name ?? "user")
                : (item.sender_name ?? "bot")}
            </span>
            <div className="mt-0.5 text-sm text-ink-dull">
              <Markdown>{item.content}</Markdown>
            </div>
          </div>
        </div>
      );
    case "branch_run": {
      const live = liveBranches[item.id];
      const selected = selection?.kind === "branch" && selection.id === item.id;
      if (live)
        return (
          <LiveBranchRunItem
            item={item as TimelineBranchRun}
            live={live}
            channelId={channelId}
            selected={selected}
            onSelect={() => onSelect({kind: "branch", id: item.id})}
          />
        );
      return (
        <BranchRunItem
          item={item as TimelineBranchRun}
          selected={selected}
          onSelect={() => onSelect({kind: "branch", id: item.id})}
        />
      );
    }
    case "worker_run": {
      const live = liveWorkers[item.id];
      const selected = selection?.kind === "worker" && selection.id === item.id;
      if (live)
        return (
          <LiveWorkerRunItem
            item={item as TimelineWorkerRun}
            live={live}
            channelId={channelId}
            selected={selected}
            onSelect={() => onSelect({kind: "worker", id: item.id})}
          />
        );
      return (
        <WorkerRunItem
          item={item as TimelineWorkerRun}
          selected={selected}
          onSelect={() => onSelect({kind: "worker", id: item.id})}
        />
      );
    }
    case "checkpoint":
      return <CheckpointItem item={item as TimelineCheckpoint} />;
  }
}

function processFallback(
  selection: ProcessSelection,
  timeline: TimelineItem[],
  workers: Record<string, ActiveWorker>,
  branches: Record<string, ActiveBranch>,
  channelName: string | null,
): ProcessRunDisplay | null {
  const item = timeline.find(
    (candidate) =>
      candidate.id === selection.id &&
      candidate.type === `${selection.kind}_run`,
  );

  if (item?.type === "branch_run") {
    const live = branches[item.id];
    const status = live ? "running" : branchRunStatus(item.conclusion);
    return {
      kind: "branch",
      id: item.id,
      input: item.description,
      output: item.conclusion ?? null,
      status,
      channel_name: channelName,
      started_at: item.started_at,
      completed_at: item.completed_at,
      tool_calls: live?.toolCalls,
    };
  }

  if (item?.type === "worker_run") {
    const live = workers[item.id];
    return {
      kind: "worker",
      id: item.id,
      input: item.task,
      output: item.result ?? null,
      status: live ? (live.isIdle ? "idle" : "running") : item.status,
      process_type: live?.workerType,
      channel_name: channelName,
      started_at: item.started_at,
      completed_at: item.completed_at,
      tool_calls: live?.toolCalls,
      interactive: live?.interactive,
    };
  }

  return null;
}

export function ChannelDetail({
  agentId,
  channelId,
  channel,
  liveState,
  onLoadMore,
}: ChannelDetailProps) {
  const timeline = liveState?.timeline ?? [];
  const hasMore = liveState?.hasMore ?? false;
  const loadingMore = liveState?.loadingMore ?? false;
  const isTyping = liveState?.isTyping ?? false;
  const thinking = liveState?.thinking ?? null;
  const workers = liveState?.workers ?? {};
  const branches = liveState?.branches ?? {};
  const activeWorkerCount = Object.keys(workers).length;
  const activeBranchCount = Object.keys(branches).length;
  const compaction = liveState?.compaction;
  const hasActivity =
    activeWorkerCount > 0 || activeBranchCount > 0 || compaction !== null;
  const [inspectOpen, setInspectOpen] = useState(false);
  const [selection, setSelection] = useState<ProcessSelection | null>(null);
  const { liveTranscripts } = useLiveContext();
  const selectedFallback = useMemo(
    () =>
      selection
        ? processFallback(
            selection,
            timeline,
            workers,
            branches,
            channel?.display_name ?? null,
          )
        : null,
    [selection, timeline, workers, branches, channel?.display_name],
  );

  const scrollRef = useRef<HTMLDivElement>(null);
  const sentinelRef = useRef<HTMLDivElement>(null);
  const lastLoadMoreAtRef = useRef(0);

  // Trigger load when the sentinel at the top of the timeline becomes visible
  const handleIntersection = useCallback(
    (entries: IntersectionObserverEntry[]) => {
      const entry = entries[0];
      if (!entry?.isIntersecting) {
        return;
      }
      const now = Date.now();
      if (now - lastLoadMoreAtRef.current < 800) {
        return;
      }
      if (hasMore && !loadingMore) {
        lastLoadMoreAtRef.current = now;
        onLoadMore();
      }
    },
    [hasMore, loadingMore, onLoadMore],
  );

  useEffect(() => {
    const sentinel = sentinelRef.current;
    if (!sentinel) return;
    const observer = new IntersectionObserver(handleIntersection, {
      root: scrollRef.current,
      rootMargin: "200px",
    });
    observer.observe(sentinel);
    return () => observer.disconnect();
  }, [handleIntersection]);

  return (
    <div className="relative flex h-full min-w-0">
      {/* Main channel content */}
      <div className="flex min-w-0 flex-1 flex-col overflow-hidden">
        {/* Channel sub-header */}
        <div className="flex h-12 items-center gap-3 border-b border-app-line/50 bg-app-dark-box/20 px-6">
          <Link
            to="/agents/$agentId/channels"
            params={{ agentId }}
            className="text-tiny text-ink-faint hover:text-ink-dull"
          >
            Channels
          </Link>
          <span className="text-ink-faint/50">/</span>
          <span className="text-sm font-medium text-ink">
            {channel?.display_name ?? channelId}
            {channel?.display_name && (
              <span className="ml-2 font-normal text-ink-faint text-tiny">
                {channelId}
              </span>
            )}
          </span>
          {channel && (
            <span
              className={`inline-flex items-center rounded-md px-1.5 py-0.5 text-tiny font-medium ${platformColor(channel.platform)}`}
            >
              {platformIcon(channel.platform)}
            </span>
          )}

          {/* Right side: activity indicators + typing + inspect */}
          <div className="ml-auto flex items-center gap-3">
            {hasActivity && (
              <div className="flex items-center gap-2">
                {activeWorkerCount > 0 && (
                  <div className="flex items-center gap-1.5">
                    <div className="h-1.5 w-1.5 animate-pulse rounded-full bg-status-warning" />
                    <span className="text-tiny text-status-warning">
                      {activeWorkerCount} worker
                      {activeWorkerCount !== 1 ? "s" : ""}
                    </span>
                  </div>
                )}
                {activeBranchCount > 0 && (
                  <div className="flex items-center gap-1.5">
                    <div className="h-1.5 w-1.5 animate-pulse rounded-full bg-accent" />
                    <span className="text-tiny text-accent-faint">
                      {activeBranchCount} branch
                      {activeBranchCount !== 1 ? "es" : ""}
                    </span>
                  </div>
                )}
                {compaction && (
                  <div className="flex items-center gap-1.5">
                    <div className="h-1.5 w-1.5 animate-pulse rounded-full bg-cyan-400" />
                    <span className="text-tiny text-cyan-300">
                      {compaction.kind === "chronicle"
                        ? "Chronicle"
                        : "Compacting"}
                    </span>
                  </div>
                )}
              </div>
            )}
            {isTyping && (
              <div className="flex items-center gap-1">
                <span className="inline-block h-1.5 w-1.5 animate-pulse rounded-full bg-accent" />
                <span className="inline-block h-1.5 w-1.5 animate-pulse rounded-full bg-accent [animation-delay:0.2s]" />
                <span className="inline-block h-1.5 w-1.5 animate-pulse rounded-full bg-accent [animation-delay:0.4s]" />
                <span className="ml-1 text-tiny text-ink-faint">typing</span>
              </div>
            )}
            <Button
              aria-label="Inspect prompt"
              onClick={() => setInspectOpen(true)}
              variant="bare"
              size="icon"
              title="Inspect prompt"
            >
              <Code className="h-4 w-4" />
            </Button>
          </div>
        </div>

        {/* Timeline — flex-col-reverse keeps scroll pinned to bottom */}
        <div
          ref={scrollRef}
          className="flex flex-1 flex-col-reverse overflow-y-auto"
        >
          <div className="flex flex-col gap-1 p-6">
            {/* Sentinel for infinite scroll — sits above the oldest item */}
            <div ref={sentinelRef} className="h-px" />
            {loadingMore && (
              <div className="flex justify-center py-3">
                <span className="text-tiny text-ink-faint">
                  Loading older messages...
                </span>
              </div>
            )}
            {!hasMore && timeline.length > 0 && (
              <div className="flex justify-center py-3">
                <span className="text-tiny text-ink-faint/50">
                  Beginning of conversation
                </span>
              </div>
            )}
            {timeline.length === 0 ? (
              <p className="text-sm text-ink-faint">No messages yet</p>
            ) : (
              timeline.map((item) => (
                <TimelineEntry
                  key={item.id}
                  item={item}
                  liveWorkers={workers}
                  liveBranches={branches}
                  channelId={channelId}
                  selection={selection}
                  onSelect={setSelection}
                />
              ))
            )}
            {thinking && (
              <div className="flex gap-3 px-3 py-2">
                <span className="flex-shrink-0 pt-0.5 text-tiny text-ink-faint">
                  {formatTimestamp(Date.now())}
                </span>
                <div className="min-w-0 flex-1 rounded-md bg-app-dark-box/40 px-3 py-2">
                  <span className="text-tiny font-medium uppercase tracking-wide text-ink-faint">
                    thinking
                  </span>
                  <div className="mt-0.5 whitespace-pre-wrap break-words text-xs italic leading-relaxed text-ink-dull">
                    {thinking}
                  </div>
                </div>
              </div>
            )}
            {isTyping && (
              <div className="flex gap-3 px-3 py-2">
                <span className="flex-shrink-0 pt-0.5 text-tiny text-ink-faint">
                  {formatTimestamp(Date.now())}
                </span>
                <div className="flex items-center gap-1.5">
                  <span className="text-sm font-medium text-status-success">
                    bot
                  </span>
                  <span className="inline-block h-1.5 w-1.5 animate-pulse rounded-full bg-ink-faint" />
                  <span className="inline-block h-1.5 w-1.5 animate-pulse rounded-full bg-ink-faint [animation-delay:0.2s]" />
                  <span className="inline-block h-1.5 w-1.5 animate-pulse rounded-full bg-ink-faint [animation-delay:0.4s]" />
                </div>
              </div>
            )}
          </div>
        </div>
      </div>

      {selection && selectedFallback && (
        <aside className="absolute inset-0 z-30 border-l border-app-line/50 md:static md:z-auto md:w-[min(46%,560px)] md:flex-shrink-0">
          <ProcessDetail
            agentId={agentId}
            selection={selection}
            fallback={selectedFallback}
            liveTranscript={liveTranscripts[selection.id]}
            onClose={() => setSelection(null)}
          />
        </aside>
      )}

      <PromptInspectModal
        open={inspectOpen}
        onOpenChange={setInspectOpen}
        channelId={channelId}
      />
    </div>
  );
}
