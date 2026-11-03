// Center chat panel: header, message list, composer.
// Typewriter animation (A3, port of ChatTranscript.cs StartStream —
// 16 ms tick, 3 chars/tick, cursor ▊), thinking card (A4),
// Stop button (A12), welcome screen (A17), friendly tool cards (A5),
// notice cards (A6), stick-to-bottom (A11), paste/drag-drop images (K3).
import { useEffect, useRef, useState } from "react";
import {
  ArrowDown,
  Brain,
  ChevronDown,
  ChevronRight,
  Copy,
  Check,
  FolderOpen,
  LogIn,
  Paperclip,
  Play,
  Send,
  Settings,
  Square,
  TerminalSquare,
  User,
  X,
} from "lucide-react";
import { Button } from "@/components/ui/button";
import type {
  Attachment,
  ChatMessage,
  Conversation,
  PendingApproval,
  ToolActionState,
  ToolPlanAction,
} from "@/lib/types";
import { formatTime } from "@/lib/types";
import { t, tf, type Lang } from "@/lib/i18n";
import { typewriterStep } from "@/lib/typewriter";
import { toolIcon, toolSummary } from "@/lib/toolSummaries";
import { MessageView } from "./MessageView";

interface Props {
  lang: Lang;
  conversation: Conversation | null;
  streaming: boolean;
  streamingText: string;
  /** Ref App reads when the user hits Stop: the text actually typed so
   * far. Stop must save that, not the full streamed text, or the message
   * would jump ahead of what the user saw. Written on every tick/reset. */
  displayedTextRef?: { current: string };
  streamingPlan: ToolPlanAction[] | null;
  /** Live agent actions for the message currently streaming (or last turn). */
  streamingActions: ToolActionState[] | null;
  /** Live thinking trail for the current turn (A4). */
  streamingThoughts: string[];
  /** True while a /goal turn is running (A15). */
  goalActive: boolean;
  pendingApproval: PendingApproval | null;
  onApprove: (approvalId: string, approved: boolean) => void;
  /** Deny + cancel the whole turn (J1 "Stop turn"). */
  onAbortApproval: (approvalId: string) => void;
  /** Stop the running turn (A12). */
  onStop: () => void;
  /** Switch the right panel to the Terminal tab (A5 "View terminal"). */
  onViewTerminal: () => void;
  /** Open the native folder picker (A17 CTA). */
  onChooseProject: () => void;
  backendHealthy: boolean;
  /** Real account-connection state (App: GET /admin/accounts judged by
   * hasSavedLogin) — drives the welcome pill. False while the list is
   * still loading or the load failed, so the pill never claims
   * "Connected" on backend health alone. */
  accountConnected: boolean;
  /** True while the backend is (auto-)starting and has not failed yet —
   * the banner shows "starting…" instead of "unreachable" + Start. */
  backendStarting: boolean;
  hasWorkspace: boolean;
  onStartBackend: () => void;
  onOpenSettings: () => void;
  onOpenLogin: () => void;
  onSend: (text: string, attachments: Attachment[]) => boolean | Promise<boolean>;
  /** Draft restored after a failed send (A16) — applied once. */
  failedDraft: { text: string; at: number } | null;
  /** Text to prefill the composer with, e.g. "Ask Muse" (C9). */
  composerPrefill: { text: string; at: number } | null;
  model: string;
}

function statusColor(status: ToolActionState["status"]): string {
  switch (status) {
    case "done":
      return "text-emerald-400";
    case "error":
      return "text-red-400";
    case "awaiting_approval":
      return "text-amber-300";
    case "denied":
      return "text-red-300";
    case "running":
      return "text-[#8aa8ff]";
    default:
      return "text-[#71717A]";
  }
}

/** Thinking card ala Codex (A4): title + live timer, thought list,
 * auto-collapsed once done ("Thought for Xs"), click to fold/unfold. */
function ThinkingCard({
  thoughts,
  lang,
  running,
  startedAt,
  doneSeconds,
}: {
  thoughts: string[];
  lang: Lang;
  running: boolean;
  startedAt: number | null;
  doneSeconds?: number;
}) {
  const [open, setOpen] = useState(true);
  const [now, setNow] = useState(Date.now());
  // Auto-collapse when the turn finishes, like ThinkingCard.Complete().
  const wasRunning = useRef(running);
  useEffect(() => {
    if (wasRunning.current && !running) setOpen(false);
    wasRunning.current = running;
  }, [running]);
  useEffect(() => {
    if (!running) return;
    const t = window.setInterval(() => setNow(Date.now()), 100);
    return () => window.clearInterval(t);
  }, [running]);
  const seconds = doneSeconds ?? (startedAt ? Math.max(0, (now - startedAt) / 1000) : 0);
  const last = thoughts.length > 0 ? thoughts[thoughts.length - 1] : null;
  return (
    <div className="mb-2 overflow-hidden rounded-lg border border-[#343541] bg-[#17171D]" role="group" aria-label={t(lang, "thinkingTitle")}>
      <button
        onClick={() => setOpen(!open)}
        className="flex w-full items-center gap-2 px-3 py-2 text-left text-[12px] font-medium text-[#A1A1AA] hover:text-white"
        aria-expanded={open}
      >
        {open ? <ChevronDown className="h-4 w-4" /> : <ChevronRight className="h-4 w-4" />}
        <Brain className="h-4 w-4 text-[#8aa8ff]" />
        <span>
          {running
            ? `${t(lang, "thinkingTitle")}… ${seconds.toFixed(1)}s`
            : tf(lang, "thoughtFor", seconds.toFixed(1))}
        </span>
        {running && <span className="h-2 w-2 animate-pulse rounded-full bg-[#3867F5]" />}
        {last && !open && <span className="min-w-0 flex-1 truncate text-[11px] text-[#71717A]">{last}</span>}
      </button>
      {open && thoughts.length > 0 && (
        <ul className="space-y-1 border-t border-[#343541] px-3 py-2">
          {thoughts.map((th, i) => (
            <li key={i} className="text-[12px] leading-relaxed text-[#A1A1AA]">
              <span className="mr-1.5 text-[#71717A]">›</span>
              {th}
            </li>
          ))}
        </ul>
      )}
    </div>
  );
}

/** Actions executed by the agent loop — live status per action, with
 * Approve/Deny buttons while an action waits for approval (Ask mode)
 * plus "Stop turn" (J1) and a "View terminal" link for commands (A5). */
function AgentActionsSection({
  actions,
  lang,
  pendingApproval,
  onApprove,
  onAbortApproval,
  onViewTerminal,
}: {
  actions: ToolActionState[];
  lang: Lang;
  pendingApproval: PendingApproval | null;
  onApprove: (approvalId: string, approved: boolean) => void;
  onAbortApproval: (approvalId: string) => void;
  onViewTerminal: () => void;
}) {
  const [open, setOpen] = useState(true);
  return (
    <div className="mt-2 overflow-hidden rounded-lg border border-[#343541] bg-[#17171D]">
      <button
        onClick={() => setOpen(!open)}
        className="flex w-full items-center gap-2 px-3 py-2 text-left text-[12px] font-medium text-[#A1A1AA] hover:text-white"
        aria-expanded={open}
      >
        {open ? <ChevronDown className="h-4 w-4" /> : <ChevronRight className="h-4 w-4" />}
        <TerminalSquare className="h-4 w-4" />
        {t(lang, "agentActions")} ({actions.length})
        <span className="ml-auto rounded bg-emerald-500/20 px-1.5 py-0.5 text-[10px] font-semibold text-emerald-300">
          {t(lang, "liveBadge")}
        </span>
      </button>
      {open && (
        <div className="border-t border-[#343541] px-3 py-2">
          <p className="mb-2 text-[11px] text-[#71717A]">{t(lang, "agentActionsNote")}</p>
          <ul className="space-y-1.5">
            {actions.map((a, i) => {
              const done = a.status === "done" || a.status === "error";
              const summary = toolSummary(a.tool, a.path, a.description, done, lang);
              const showDesc = a.description && a.description !== summary;
              return (
                <li key={`${a.tool}-${i}`} className="rounded-md border border-[#343541] bg-[#292A34] px-2.5 py-2">
                  <div className="flex items-center gap-2">
                    <span aria-hidden className="text-[13px]">{toolIcon(a.tool)}</span>
                    <span className="min-w-0 flex-1 truncate text-[12px] text-[#F5F5F7]">{summary}</span>
                    <span className={`shrink-0 text-[11px] font-medium ${statusColor(a.status)}`}>
                      {a.status === "running"
                        ? t(lang, "agentRunning")
                        : a.status === "awaiting_approval"
                          ? t(lang, "agentAwaitingApproval")
                          : a.status === "done"
                            ? t(lang, "agentDone")
                            : a.status === "denied"
                              ? t(lang, "agentDenied")
                              : a.status}
                    </span>
                  </div>
                  <div className="mt-0.5 flex items-center gap-2 pl-6 text-[11px] text-[#71717A]">
                    <span className="font-mono">{a.tool}</span>
                    {a.path && <span className="min-w-0 flex-1 truncate font-mono">{a.path}</span>}
                  </div>
                  {showDesc && <p className="mt-1 pl-6 text-[11px] leading-relaxed text-[#A1A1AA]">{a.description}</p>}
                  {a.preview && (
                    <div className="mt-1.5 pl-6">
                      <p className="mb-1 text-[11px] font-medium text-[#A1A1AA]">{t(lang, "toolPreview")}</p>
                      <pre className="max-h-48 overflow-auto whitespace-pre-wrap rounded border border-[#343541] bg-[#17171D] p-2 font-mono text-[11px] leading-relaxed text-[#A1A1AA]">
                        {a.preview.slice(0, 4000)}
                      </pre>
                    </div>
                  )}
                  {a.result && (
                    <pre className="mt-1 max-h-32 overflow-auto whitespace-pre-wrap pl-6 font-mono text-[11px] leading-relaxed text-[#71717A]">
                      {a.result.slice(0, 2000)}
                    </pre>
                  )}
                  <div className="mt-1.5 flex gap-2 pl-6">
                    {a.tool.toLowerCase().startsWith("run_command") && (
                      <button onClick={onViewTerminal} className="text-[11px] font-medium text-[#8aa8ff] hover:text-white">
                        {t(lang, "viewTerminal")} →
                      </button>
                    )}
                    {a.status === "awaiting_approval" && pendingApproval?.approvalId === a.approvalId && (
                      <>
                        <Button size="sm" className="h-7 bg-emerald-600 text-white hover:bg-emerald-500" onClick={() => onApprove(a.approvalId!, true)}>
                          {t(lang, "approve")}
                        </Button>
                        <Button size="sm" variant="outline" className="h-7 border-[#343541] text-[#A1A1AA]" onClick={() => onApprove(a.approvalId!, false)}>
                          {t(lang, "deny")}
                        </Button>
                        <Button
                          size="sm"
                          variant="outline"
                          className="h-7 border-red-500/40 text-red-300 hover:bg-red-500/10"
                          onClick={() => onAbortApproval(a.approvalId!)}
                          title={t(lang, "abortTurn")}
                        >
                          {t(lang, "abortTurn")}
                        </Button>
                      </>
                    )}
                  </div>
                </li>
              );
            })}
          </ul>
        </div>
      )}
    </div>
  );
}

/** Legacy display-only plan (plain-chat fallback when /v1/agent/* is absent). */
function ToolPlanSection({ actions, lang }: { actions: ToolPlanAction[]; lang: Lang }) {
  const [open, setOpen] = useState(true);
  return (
    <div className="mt-2 overflow-hidden rounded-lg border border-[#343541] bg-[#17171D]">
      <button
        onClick={() => setOpen(!open)}
        className="flex w-full items-center gap-2 px-3 py-2 text-left text-[12px] font-medium text-[#A1A1AA] hover:text-white"
        aria-expanded={open}
      >
        {open ? <ChevronDown className="h-4 w-4" /> : <ChevronRight className="h-4 w-4" />}
        <TerminalSquare className="h-4 w-4" />
        {t(lang, "toolPlan")} ({actions.length})
        <span className="ml-auto rounded bg-amber-500/20 px-1.5 py-0.5 text-[10px] font-semibold text-amber-300">
          {t(lang, "stubBadge")} — plan only
        </span>
      </button>
      {open && (
        <div className="border-t border-[#343541] px-3 py-2">
          <p className="mb-2 text-[11px] text-[#71717A]">{t(lang, "toolPlanNote")}</p>
          <pre className="max-h-48 overflow-auto whitespace-pre-wrap font-mono text-[11px] leading-relaxed text-[#A1A1AA]">
            {JSON.stringify(actions, null, 2)}
          </pre>
        </div>
      )}
    </div>
  );
}

/** Notice card for a finished-but-not-ok turn (A6): cancelled /
 * declined / incomplete — distinct from a red error box. */
function NoticeCard({ msg, lang }: { msg: ChatMessage; lang: Lang }) {
  const text =
    msg.turnStatus === "cancelled"
      ? t(lang, "turnCancelled")
      : msg.turnStatus === "declined"
        ? t(lang, "turnDeclined")
        : msg.turnStatus === "incomplete"
          ? t(lang, "turnIncomplete")
          : null;
  if (!text) return null;
  return (
    <div className="mb-2 rounded-lg border border-amber-500/40 bg-amber-500/10 px-3 py-2 text-[13px] text-amber-200" role="status">
      {text}
    </div>
  );
}

function MessageRow({
  msg,
  lang,
  isStreamingTarget,
  isTypingTarget,
  streamingText,
  streamingPlan,
  streamingActions,
  streamingThoughts,
  turnStartedAt,
  showCursor,
  pendingApproval,
  onApprove,
  onAbortApproval,
  onViewTerminal,
}: {
  msg: ChatMessage;
  lang: Lang;
  isStreamingTarget: boolean;
  /** True while the typewriter is still typing this message — stays true
   * briefly after the turn finished, until displayed catches up with the
   * saved content. Only the message TEXT follows it; plan/actions/
   * thoughts/thinking-card still follow isStreamingTarget, because the
   * live streaming props are cleared at finish() and the saved values
   * must take over immediately (otherwise the tool cards would flicker
   * away during the catch-up typing). */
  isTypingTarget: boolean;
  streamingText: string;
  streamingPlan: ToolPlanAction[] | null;
  streamingActions: ToolActionState[] | null;
  streamingThoughts: string[];
  turnStartedAt: number | null;
  showCursor: boolean;
  pendingApproval: PendingApproval | null;
  onApprove: (approvalId: string, approved: boolean) => void;
  onAbortApproval: (approvalId: string) => void;
  onViewTerminal: () => void;
}) {
  const [copied, setCopied] = useState(false);
  // The message text follows the typewriter (keeps typing after the turn
  // finished); everything else follows the live streaming flag.
  const content = isTypingTarget ? streamingText : msg.content;
  const plan = isStreamingTarget ? streamingPlan : msg.toolPlan;
  const actions = isStreamingTarget ? streamingActions : msg.agentActions;
  const thoughts = isStreamingTarget ? streamingThoughts : (msg.thoughts ?? []);
  const isUser = msg.role === "user";
  const copy = async () => {
    try {
      await navigator.clipboard.writeText(content);
      setCopied(true);
      setTimeout(() => setCopied(false), 1500);
    } catch {
      /* clipboard unavailable */
    }
  };

  if (isUser) {
    return (
      <div className="flex justify-end px-4 py-2">
        <div className="max-w-[75%] rounded-2xl rounded-br-sm bg-[#292A34] px-4 py-3">
          <div className="mb-1 flex items-center gap-2 text-[12px] text-[#A1A1AA]">
            <User className="h-3.5 w-3.5" /> {t(lang, "you")} · {formatTime(msg.createdAt)}
          </div>
          <MessageView content={content} />
          {msg.attachments && msg.attachments.length > 0 && (
            <div className="mt-2 flex flex-wrap gap-1.5">
              {msg.attachments.map((a, i) =>
                a.kind === "image" && a.dataUrl ? (
                  <img key={i} src={a.dataUrl} alt={a.name} title={a.name} className="h-14 w-14 rounded-md border border-[#343541] object-cover" />
                ) : (
                  <span key={i} className="text-[11px] text-[#71717A]">[{a.name}]</span>
                ),
              )}
            </div>
          )}
        </div>
      </div>
    );
  }

  return (
    <div className="px-4 py-3">
      <div className="flex items-center gap-2">
        <span className="flex h-7 w-7 items-center justify-center rounded-full bg-[#3867F5] text-[13px] font-bold text-white">
          M
        </span>
        <span className="text-[13px] font-semibold text-[#F5A623]">{t(lang, "museAgent")}</span>
        <span className="text-[11px] text-[#71717A]">{formatTime(msg.createdAt)}</span>
        <button onClick={copy} className="ml-1 text-[#71717A] hover:text-white" title={t(lang, "copy")} aria-label={t(lang, "copy")}>
          {copied ? <Check className="h-3.5 w-3.5 text-emerald-400" /> : <Copy className="h-3.5 w-3.5" />}
        </button>
      </div>
      <div className="pl-9 pt-1.5 text-[14px] leading-relaxed text-[#F5F5F7]">
        {(thoughts.length > 0 || isStreamingTarget) && (
          <ThinkingCard
            thoughts={thoughts}
            lang={lang}
            running={isStreamingTarget}
            startedAt={isStreamingTarget ? turnStartedAt : null}
            doneSeconds={msg.thinkingSeconds}
          />
        )}
        {msg.error ? (
          <div className="rounded-lg border border-red-500/40 bg-red-500/10 px-3 py-2 text-[13px] text-red-300" role="alert">
            {msg.error}
          </div>
        ) : (
          <>
            {msg.turnStatus && <NoticeCard msg={msg} lang={lang} />}
            <MessageView content={content} />
            {isTypingTarget && showCursor && (
              <span className="typewriter-cursor" aria-hidden>▊</span>
            )}
          </>
        )}
        {actions && actions.length > 0 && (
          <AgentActionsSection
            actions={actions}
            lang={lang}
            pendingApproval={pendingApproval}
            onApprove={onApprove}
            onAbortApproval={onAbortApproval}
            onViewTerminal={onViewTerminal}
          />
        )}
        {plan && plan.length > 0 && <ToolPlanSection actions={plan} lang={lang} />}
        {msg.tokensIn !== undefined && msg.tokensOut !== undefined && (
          <div className="mt-2 text-[11px] text-[#71717A]">
            {(msg.tokensIn / 1000).toFixed(1)}k in · {(msg.tokensOut / 1000).toFixed(1)}k out
          </div>
        )}
      </div>
    </div>
  );
}

// --- Typewriter (A3) ------------------------------------------------------
// WinForms ChatTranscript: 16 ms timer, 3 chars/tick. The displayed text
// chases the target text; the tick itself lives in @/lib/typewriter so
// it can be unit-tested in Node (see scripts/test-typewriter.mjs).

const SUGGESTIONS = [
  { title: "suggestExplore", prompt: "suggestExplorePrompt", desc: "suggestExploreDesc" },
  { title: "suggestFix", prompt: "suggestFixPrompt", desc: "suggestFixDesc" },
  { title: "suggestFeature", prompt: "suggestFeaturePrompt", desc: "suggestFeatureDesc" },
  { title: "suggestQuality", prompt: "suggestQualityPrompt", desc: "suggestQualityDesc" },
] as const;

const MAX_IMAGES = 4;
const MAX_IMAGE_BYTES = 8 * 1024 * 1024;
const MAX_IMAGES_TOTAL_BYTES = 16 * 1024 * 1024;

export function ChatPanel(props: Props) {
  const { lang, conversation } = props;
  const [draft, setDraft] = useState("");
  const [attachments, setAttachments] = useState<Attachment[]>([]);
  const [copiedAll, setCopiedAll] = useState(false);
  const [dragOver, setDragOver] = useState(false);
  const [stickToBottom, setStickToBottom] = useState(true);
  const [showJump, setShowJump] = useState(false);
  const fileRef = useRef<HTMLInputElement>(null);
  const listRef = useRef<HTMLDivElement>(null);
  const taRef = useRef<HTMLTextAreaElement>(null);

  const messages = conversation?.messages ?? [];
  const lastAssistant = [...messages].reverse().find((m) => m.role === "assistant");

  // --- Typewriter state: displayed text chases the target text. ---
  const [displayed, setDisplayed] = useState("");
  const displayedRef = useRef("");
  // The chase target: live streamingText while the turn runs; once the
  // turn finished, finish()/stopTurn() clear streamingText and save the
  // full text into the assistant message, so the target becomes that
  // saved content. (The old code jumped displayed straight to the full
  // text when streaming flipped off — a single-delta reply therefore
  // appeared all at once instead of typing.)
  const typewriterTarget = props.streaming
    ? props.streamingText
    : (lastAssistant?.content ?? "");
  const targetRef = useRef(typewriterTarget);
  targetRef.current = typewriterTarget;
  const streamingRef = useRef(props.streaming);
  streamingRef.current = props.streaming;
  // Which message the animation belongs to. Set when a turn starts; a
  // conversation switch clears it, so an animation never leaks into
  // another conversation's last message.
  const animatedIdRef = useRef<string | null>(null);
  const setDisplayedBoth = (text: string) => {
    displayedRef.current = text;
    if (props.displayedTextRef) props.displayedTextRef.current = text;
    setDisplayed(text);
  };
  useEffect(() => {
    if (props.streaming) {
      // New turn: start from empty and remember which message we animate.
      // The placeholder assistant message is already in `messages` here
      // (App adds it before setStreaming(true)), so lastAssistant is it.
      setDisplayedBoth("");
      animatedIdRef.current = lastAssistant?.id ?? null;
    }
    // The interval is created ONCE per turn (re-created on the streaming
    // flip, but the displayed text is preserved across it) and reads the
    // moving target through targetRef — re-creating it on every
    // streamingText change would reset the 16 ms timer faster than it
    // can fire while deltas pour in. When streaming ends the interval
    // is NOT stopped: it keeps typing until displayed has caught up with
    // the target, then stops itself. Finishing is driven by reaching the
    // target, never by the streaming flag — so a reply that arrived in a
    // single delta still types out live (~0.5 s for ~90 chars) instead
    // of appearing whole.
    const id = window.setInterval(() => {
      const target = targetRef.current;
      const cur = displayedRef.current;
      if (cur.length >= target.length) {
        if (!streamingRef.current) {
          window.clearInterval(id);
        }
        return;
      }
      const next = typewriterStep(target, cur, streamingRef.current);
      // typewriterStep returns cur unchanged while holding at an
      // unclosed fence mid-stream — just wait for the fence to close.
      if (next.length <= cur.length) return;
      setDisplayedBoth(next);
    }, 16);
    return () => window.clearInterval(id);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [props.streaming]);

  // A switch to another conversation abandons this animation: reset the
  // displayed text and forget the animated message id. The interval above
  // is deliberately left running (killing it would break the animation
  // if the turn is still streaming in the original conversation); its
  // output is invisible elsewhere because isTyping below requires the
  // animated id to match this conversation's last assistant message.
  const convId = conversation?.id;
  useEffect(() => {
    displayedRef.current = "";
    if (props.displayedTextRef) props.displayedTextRef.current = "";
    animatedIdRef.current = null;
    setDisplayed("");
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [convId]);

  // True while this conversation's last assistant message is still being
  // typed — during the turn, or during the catch-up after it finished.
  const isTyping =
    (props.streaming || displayed.length < typewriterTarget.length) &&
    lastAssistant !== undefined &&
    lastAssistant.id === animatedIdRef.current;

  // Turn start (for the thinking-card timer).
  const turnStartedAtRef = useRef<number | null>(null);
  const prevStreaming = useRef(false);
  useEffect(() => {
    if (props.streaming && !prevStreaming.current) turnStartedAtRef.current = Date.now();
    if (!props.streaming) turnStartedAtRef.current = null;
    prevStreaming.current = props.streaming;
  }, [props.streaming]);

  // A16 — restore the draft when a send failed (App reports it).
  const lastFailedAt = useRef(0);
  useEffect(() => {
    if (props.failedDraft && props.failedDraft.at > lastFailedAt.current) {
      lastFailedAt.current = props.failedDraft.at;
      setDraft((d) => (d.trim() ? d : props.failedDraft!.text));
    }
  }, [props.failedDraft]);

  // C9 — prefill the composer (e.g. "Ask Muse" from the editor).
  const lastPrefillAt = useRef(0);
  useEffect(() => {
    if (props.composerPrefill && props.composerPrefill.at > lastPrefillAt.current) {
      lastPrefillAt.current = props.composerPrefill.at;
      setDraft(props.composerPrefill.text);
      taRef.current?.focus();
    }
  }, [props.composerPrefill]);

  // A11 — stick-to-bottom: only auto-scroll while the user is already
  // near the bottom (120 px tolerance, like IsNearBottom()).
  const isNearBottom = () => {
    const el = listRef.current;
    if (!el) return true;
    return el.scrollHeight - el.scrollTop - el.clientHeight <= 120;
  };
  const onScroll = () => {
    const near = isNearBottom();
    setStickToBottom(near);
    setShowJump(!near);
  };
  useEffect(() => {
    const el = listRef.current;
    if (el && stickToBottom) el.scrollTop = el.scrollHeight;
  }, [messages.length, displayed, props.streaming, stickToBottom]);
  const jumpToBottom = () => {
    const el = listRef.current;
    if (el) {
      el.scrollTop = el.scrollHeight;
      setStickToBottom(true);
      setShowJump(false);
    }
  };

  // Auto-resize composer.
  useEffect(() => {
    const el = taRef.current;
    if (el) {
      el.style.height = "auto";
      el.style.height = `${Math.min(el.scrollHeight, 160)}px`;
    }
  }, [draft]);

  const canSend = draft.trim().length > 0 && !props.streaming;

  const send = async () => {
    // NOTE: do NOT require `conversation` here — App.sendMessage creates
    // a new conversation when none is active. Requiring it made Enter/send
    // silently do nothing on first launch ("No conversations yet").
    if (!canSend) return;
    const text = draft.trim();
    const atts = attachments;
    // Clear the composer IMMEDIATELY so the prompt does not linger while
    // Muse thinks (onSend only resolves after the whole turn finishes).
    // If App rejects the send (returns false — no Muse account connected,
    // the login dialog was opened instead), put the draft AND the
    // attachments back so nothing the user typed is lost; only restore
    // into an empty composer so text typed meanwhile is never clobbered.
    setDraft("");
    setAttachments([]);
    const accepted = await props.onSend(text, atts);
    if (accepted === false) {
      setDraft((d) => (d.trim() ? d : text));
      setAttachments((a) => (a.length > 0 ? a : atts));
    }
  };

  const onAttach = async (files: FileList | File[] | null) => {
    if (!files) return;
    const next: Attachment[] = [];
    // WinForms cap (Images.cs): max 4 images, 8 MB each, 16 MB total.
    const imageCount = (list: Attachment[]) => list.filter((a) => a.kind === "image").length;
    const imageBytes = (list: Attachment[]) =>
      list.filter((a) => a.kind === "image").reduce((n, a) => n + (a.size ?? 0), 0);
    for (const f of Array.from(files)) {
      if (f.type.startsWith("image/")) {
        if (imageCount([...attachments, ...next]) >= MAX_IMAGES) continue;
        if (imageBytes([...attachments, ...next]) + f.size > MAX_IMAGES_TOTAL_BYTES) continue;
        if (f.size <= MAX_IMAGE_BYTES) {
          try {
            const dataUrl = await new Promise<string>((resolve, reject) => {
              const r = new FileReader();
              r.onload = () => resolve(String(r.result));
              r.onerror = () => reject(new Error("read failed"));
              r.readAsDataURL(f);
            });
            next.push({ name: f.name, text: null, kind: "image", dataUrl, size: f.size });
          } catch {
            next.push({ name: f.name, text: null, kind: "image", size: f.size });
          }
        } else {
          next.push({ name: f.name, text: null, kind: "image", size: f.size });
        }
      } else if (f.type.startsWith("text/") || /\.(md|txt|json|ts|tsx|js|css|html|py|rs|cs|yaml|yml|toml|csv)$/i.test(f.name)) {
        try {
          const text = await f.text();
          next.push({ name: f.name, text: text.slice(0, 20000), kind: "text", size: f.size });
        } catch {
          next.push({ name: f.name, text: null, kind: "other", size: f.size });
        }
      } else {
        next.push({ name: f.name, text: null, kind: "other", size: f.size });
      }
    }
    setAttachments((a) => [...a, ...next].slice(0, 8));
  };

  // K3 — paste images from the clipboard (Ctrl+V) into the composer.
  const onPaste = (e: React.ClipboardEvent) => {
    const items = e.clipboardData?.items;
    if (!items) return;
    const files: File[] = [];
    for (const item of Array.from(items)) {
      if (item.type.startsWith("image/")) {
        const f = item.getAsFile();
        if (f) files.push(f);
      }
    }
    if (files.length > 0) {
      e.preventDefault();
      void onAttach(files);
    }
  };

  const copyTranscript = async () => {
    if (!conversation) return;
    const text = conversation.messages
      .map((m) => `${m.role === "user" ? t(lang, "you") : t(lang, "museAgent")}: ${m.content}`)
      .join("\n\n");
    try {
      await navigator.clipboard.writeText(text);
      setCopiedAll(true);
      setTimeout(() => setCopiedAll(false), 1500);
    } catch {
      /* ignore */
    }
  };

  const welcomeHint = !props.hasWorkspace
    ? t(lang, "welcomeHintNoProject")
    : !props.backendHealthy
      ? t(lang, "welcomeHintNoConnection")
      : t(lang, "welcomeHintReady");

  const latestThought =
    props.streamingThoughts.length > 0 ? props.streamingThoughts[props.streamingThoughts.length - 1] : null;

  return (
    <div className="flex h-full flex-col bg-[#22232C]">
      {/* Header */}
      <div className="flex min-h-12 items-center gap-2 border-b border-[#343541] px-4 py-2">
        <div className="min-w-0 flex-1">
          <h1 className="truncate text-[14px] font-semibold text-[#F5F5F7]">
            {conversation?.title ?? "Muse"}
          </h1>
          <p className="text-[11px] text-[#71717A]">
            {messages.length} {t(lang, "messages")}
          </p>
        </div>
        <span className="flex items-center gap-1 rounded-full border border-[#343541] px-2 py-1 text-[11px] text-[#A1A1AA]">
          <User className="h-3 w-3" /> {t(lang, "profileDefault")}
        </span>
        <span className="rounded-full bg-[#3867F5]/20 px-2 py-1 font-mono text-[11px] text-[#8aa8ff]">
          {props.model === "muse-chat" ? "Muse" : props.model}
        </span>
        <Button variant="ghost" size="sm" className="text-[#A1A1AA] hover:text-white" onClick={copyTranscript} title={t(lang, "exportTranscript")}>
          {copiedAll ? <Check className="h-4 w-4 text-emerald-400" /> : <Copy className="h-4 w-4" />}
          {copiedAll ? t(lang, "copied") : t(lang, "copy")}
        </Button>
        <Button variant="ghost" size="icon" className="text-[#A1A1AA] hover:text-white" onClick={props.onOpenSettings} title={t(lang, "settings")} aria-label={t(lang, "settings")}>
          <Settings className="h-4 w-4" />
        </Button>
      </div>

      {/* Backend warning — while the backend is still (auto-)starting,
        show a neutral "starting…" strip, NOT the unreachable banner +
        Start button; the unreachable banner only appears once the start
        has really failed (backend stopped / died with an error). */}
      {!props.backendHealthy && props.backendStarting ? (
        <div className="flex items-center gap-2 border-b border-[#343541] bg-[#292A34] px-4 py-2 text-[12px] text-[#A1A1AA]">
          <span className="flex-1">{t(lang, "backendStarting")}</span>
        </div>
      ) : !props.backendHealthy ? (
        <div className="flex items-center gap-2 border-b border-amber-500/30 bg-amber-500/10 px-4 py-2 text-[12px] text-amber-200">
          <span className="flex-1">{t(lang, "backendUnreachable")}</span>
          <Button size="sm" className="bg-amber-500 text-black hover:bg-amber-400" onClick={props.onStartBackend}>
            <Play className="h-3.5 w-3.5" /> {t(lang, "startBackend")}
          </Button>
        </div>
      ) : null}

      {/* Messages */}
      <div ref={listRef} onScroll={onScroll} className="relative min-h-0 flex-1 overflow-y-auto py-2" role="log" aria-label={t(lang, "messages")}>
        {messages.length === 0 && !props.streaming && (
          <div className="flex h-full flex-col items-center justify-center px-6 py-8 text-center">
            <p className="text-[11px] font-semibold uppercase tracking-[0.2em] text-[#71717A]">{t(lang, "welcomeEyebrow")}</p>
            <h2 className="mt-2 text-[20px] font-bold text-[#F5F5F7]">{t(lang, "welcomeHeading")}</h2>
            <p className="mt-2 max-w-md text-[13px] leading-relaxed text-[#A1A1AA]">{welcomeHint}</p>
            <div className="mt-4 flex flex-wrap items-center justify-center gap-2">
              {!props.hasWorkspace && (
                <Button className="bg-[#3867F5] text-white hover:bg-[#2f5ae0]" onClick={props.onChooseProject}>
                  <FolderOpen className="h-4 w-4" /> {t(lang, "welcomeChooseProject")}
                </Button>
              )}
              {props.hasWorkspace && (
                <span className="rounded-full border border-emerald-500/40 bg-emerald-500/10 px-2.5 py-1 text-[12px] text-emerald-300">
                  ✓ {t(lang, "welcomeProjectSelected")}
                </span>
              )}
              {!props.backendHealthy ? (
                <Button variant="outline" className="border-[#343541] text-[#F5F5F7]" onClick={props.onOpenLogin}>
                  <LogIn className="h-4 w-4" /> {t(lang, "welcomeConnect")}
                </Button>
              ) : props.accountConnected ? (
                <span className="rounded-full border border-emerald-500/40 bg-emerald-500/10 px-2.5 py-1 text-[12px] text-emerald-300">
                  ✓ {t(lang, "welcomeConnected")}
                </span>
              ) : (
                <>
                  <span className="rounded-full border border-amber-500/40 bg-amber-500/10 px-2.5 py-1 text-[12px] text-amber-200">
                    ✕ {t(lang, "welcomeNotConnected")}
                  </span>
                  <Button variant="outline" className="border-[#343541] text-[#F5F5F7]" onClick={props.onOpenLogin}>
                    <LogIn className="h-4 w-4" /> {t(lang, "welcomeConnect")}
                  </Button>
                </>
              )}
            </div>
            <div className="mt-6 grid w-full max-w-lg grid-cols-1 gap-2 sm:grid-cols-2">
              {SUGGESTIONS.map((s) => (
                <button
                  key={s.title}
                  onClick={() => {
                    setDraft(t(lang, s.prompt));
                    taRef.current?.focus();
                  }}
                  className="rounded-lg border border-[#343541] bg-[#292A34] px-3 py-2.5 text-left hover:border-[#3867F5] hover:bg-[#343541]"
                >
                  <span className="block text-[13px] font-semibold text-[#F5F5F7]">{t(lang, s.title)}</span>
                  <span className="mt-0.5 block text-[12px] text-[#A1A1AA]">{t(lang, s.desc)}</span>
                </button>
              ))}
            </div>
          </div>
        )}
        {messages.map((m) => (
          <MessageRow
            key={m.id}
            msg={m}
            lang={lang}
            isStreamingTarget={props.streaming && lastAssistant?.id === m.id}
            isTypingTarget={isTyping && lastAssistant?.id === m.id}
            streamingText={displayed}
            streamingPlan={props.streamingPlan}
            streamingActions={props.streamingActions}
            streamingThoughts={props.streamingThoughts}
            turnStartedAt={turnStartedAtRef.current}
            showCursor={displayed.length < typewriterTarget.length}
            pendingApproval={props.pendingApproval}
            onApprove={props.onApprove}
            onAbortApproval={props.onAbortApproval}
            onViewTerminal={props.onViewTerminal}
          />
        ))}
        {props.streaming && (
          <div className="flex items-center gap-2 px-4 py-2 text-[12px] text-[#71717A]" aria-live="polite">
            <span className="h-2 w-2 animate-pulse rounded-full bg-[#3867F5]" />
            {props.goalActive ? (
              <span className="font-medium text-[#F5A623]">🎯 {t(lang, "goalRunning")}</span>
            ) : latestThought ? (
              <span className="min-w-0 truncate">{latestThought}</span>
            ) : (
              <span>{t(lang, "thinking")}</span>
            )}
          </div>
        )}
        {showJump && (
          <button
            onClick={jumpToBottom}
            className="sticky bottom-3 left-1/2 flex -translate-x-1/2 items-center gap-1 rounded-full border border-[#343541] bg-[#292A34] px-3 py-1.5 text-[12px] text-[#A1A1AA] shadow-lg hover:text-white"
            title={t(lang, "scrollToBottom")}
          >
            <ArrowDown className="h-3.5 w-3.5" /> {t(lang, "scrollToBottom")}
          </button>
        )}
      </div>

      {/* Composer */}
      <div
        className="border-t border-[#343541] px-4 pb-3 pt-2"
        onDragOver={(e) => {
          e.preventDefault();
          setDragOver(true);
        }}
        onDragLeave={() => setDragOver(false)}
        onDrop={(e) => {
          e.preventDefault();
          setDragOver(false);
          void onAttach(e.dataTransfer?.files ?? null);
        }}
      >
        {dragOver && (
          <p className="mb-2 rounded-lg border border-dashed border-[#3867F5] bg-[#3867F5]/10 px-3 py-2 text-center text-[12px] text-[#8aa8ff]">
            {t(lang, "dragDropHint")}
          </p>
        )}
        {attachments.length > 0 && (
          <div className="mb-2 flex flex-wrap gap-1.5">
            {attachments.map((a, i) => (
              <span key={i} className="flex items-center gap-1 rounded-lg border border-[#343541] bg-[#292A34] px-1.5 py-1 text-[11px] text-[#A1A1AA]">
                {a.kind === "image" && a.dataUrl ? (
                  <img src={a.dataUrl} alt={a.name} title={a.name} className="h-10 w-10 rounded object-cover" />
                ) : (
                  <Paperclip className="h-3 w-3" />
                )}
                <span className="max-w-28 truncate">{a.name}</span>
                <button onClick={() => setAttachments(attachments.filter((_, j) => j !== i))} className="hover:text-white" aria-label={t(lang, "remove")}>
                  <X className="h-3 w-3" />
                </button>
              </span>
            ))}
            <span className="w-full text-[11px] text-[#71717A]">{t(lang, "attachNote")}</span>
          </div>
        )}
        <div className="rounded-xl border border-[#343541] bg-[#292A34] px-3 py-2">
          <textarea
            ref={taRef}
            value={draft}
            rows={1}
            onChange={(e) => setDraft(e.currentTarget.value)}
            onPaste={onPaste}
            onKeyDown={(e) => {
              if (e.key === "Enter" && !e.shiftKey) {
                e.preventDefault();
                send();
              }
            }}
            placeholder={t(lang, "messagePlaceholder")}
            aria-label={t(lang, "messagePlaceholder")}
            title={t(lang, "pasteImageHint")}
            className="max-h-40 w-full resize-none bg-transparent text-[14px] leading-relaxed text-[#F5F5F7] outline-none placeholder:text-[#71717A]"
          />
          <div className="flex items-center gap-1 pt-1">
            <Button variant="ghost" size="icon" className="h-8 w-8 text-[#A1A1AA] hover:text-white" title={t(lang, "attachedFiles")} aria-label={t(lang, "attachedFiles")} onClick={() => fileRef.current?.click()}>
              <Paperclip className="h-4 w-4" />
            </Button>
            <input ref={fileRef} type="file" multiple className="hidden" onChange={(e) => { void onAttach(e.currentTarget.files); e.currentTarget.value = ""; }} />
            <div className="flex-1" />
            {props.streaming ? (
              <Button
                size="sm"
                className="h-9 rounded-full bg-red-600 text-white hover:bg-red-500"
                onClick={props.onStop}
                title={t(lang, "stopTurnTitle")}
                aria-label={t(lang, "stopTurn")}
              >
                <Square className="h-4 w-4" /> {t(lang, "stop")}
              </Button>
            ) : (
              <Button size="icon" className="h-9 w-9 rounded-full bg-[#3867F5] text-white hover:bg-[#2f5ae0]" disabled={!canSend} onClick={send} title={t(lang, "send")} aria-label={t(lang, "send")}>
                <Send className="h-4 w-4" />
              </Button>
            )}
          </div>
        </div>
      </div>
    </div>
  );
}
