// Live Studio project progress, backed by actual Gateway TaskStore values.
// Polling persists after the assistant turn ends; no model calls/quota use.
import { useEffect, useState } from "react";
import { Check, Copy, Film, Loader2, AlertCircle } from "lucide-react";

import { getAgentStudioProgress } from "@/lib/gateway";
import {
  studioProgressPercent,
  studioProgressTerminal,
  type StudioProgressSnapshot,
} from "@/lib/studioProgress";
import type { Lang } from "@/lib/i18n";

interface Props {
  lang: Lang;
  baseUrl: string;
  apiKey: string;
  sessionId: string;
  projectId: string;
}

const STAGE_TEXT: Record<string, { en: string; id: string }> = {
  waiting: { en: "Preparing render", id: "Menyiapkan render" },
  queued: { en: "Waiting in render queue", id: "Menunggu antrean render" },
  rendering: { en: "Generating video", id: "Sedang membuat video" },
  saving: { en: "Downloading and saving result", id: "Mengunduh dan menyimpan hasil" },
  composing: { en: "Joining video scenes", id: "Menggabungkan scene video" },
  complete: { en: "Video ready", id: "Video selesai" },
  failed: { en: "Render failed", id: "Render gagal" },
  unknown: { en: "Checking render state", id: "Memeriksa status render" },
  saved: { en: "Saved", id: "Tersimpan" },
};

function stageText(stage: string, lang: Lang): string {
  return STAGE_TEXT[stage]?.[lang] ?? stage;
}

export function StudioProgressCard({ lang, baseUrl, apiKey, sessionId, projectId }: Props) {
  const [progress, setProgress] = useState<StudioProgressSnapshot | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [copied, setCopied] = useState(false);

  useEffect(() => {
    let mounted = true;
    let timer: ReturnType<typeof setTimeout> | undefined;
    const controller = new AbortController();

    const check = async () => {
      try {
        const snapshot = await getAgentStudioProgress(baseUrl, apiKey, sessionId, projectId, controller.signal);
        if (!mounted) return;
        setProgress(snapshot);
        setError(null);
        if (!studioProgressTerminal(snapshot)) {
          timer = setTimeout(check, 3500);
        }
      } catch (e) {
        if (!mounted || controller.signal.aborted) return;
        // A connection failure does not mean a provider render failed.
        setError(String(e instanceof Error ? e.message : e).slice(0, 240));
        timer = setTimeout(check, 8500);
      }
    };
    void check();
    return () => {
      mounted = false;
      if (timer) clearTimeout(timer);
      controller.abort();
    };
  }, [baseUrl, apiKey, sessionId, projectId]);

  const state = progress?.state ?? "waiting";
  const completed = state === "complete";
  const failed = state === "failed";
  const percent = progress ? studioProgressPercent(progress) : 0;
  const text = lang === "id";
  const finalPath = completed ? progress?.final_video_path : null;

  return (
    <div
      className="border-t border-[#343541] px-3 py-3 text-[12px] text-[#D4D4D8]"
      role="status"
      aria-live="polite"
      aria-label={text ? "Progres video" : "Video progress"}
      data-studio-project={projectId}
    >
      <div className="flex flex-wrap items-center justify-between gap-2">
        <span className="flex min-w-0 items-center gap-2 font-semibold text-[#F5F5F7]">
          <Film className="h-4 w-4 shrink-0 text-[#8aa8ff]" aria-hidden />
          Live video rendering
        </span>
        <span className={"flex items-center gap-1.5 font-medium " + (failed ? "text-red-300" : completed ? "text-emerald-400" : "text-[#8aa8ff]")}>
          {completed ? <Check className="h-4 w-4" aria-hidden /> : failed ? <AlertCircle className="h-4 w-4" aria-hidden /> : <Loader2 className="h-3.5 w-3.5 animate-spin" aria-hidden />}
          {stageText(state, lang)}
        </span>
      </div>
      <p className="mt-1 truncate text-[11px] text-[#71717A]" title={progress?.name || projectId}>
        {progress?.name || projectId}
      </p>
      <div className="mt-3 flex items-center justify-between">
        <span className="text-[11px] text-[#A1A1AA]">
          {text ? "Progres dari gateway" : "Gateway-reported progress"}
        </span>
        <strong className="font-mono text-[13px] tabular-nums text-[#F5F5F7]">{percent}%</strong>
      </div>
      <div
        className="mt-1 h-2 overflow-hidden rounded-full bg-[#343541]"
        role="progressbar"
        aria-label={text ? "Progres produksi video" : "Video generation progress"}
        aria-valuemin={0}
        aria-valuemax={100}
        aria-valuenow={percent}
      >
        <div
          className={"h-full rounded-full transition-[width] duration-500 " + (failed ? "bg-red-500" : completed ? "bg-emerald-500" : "bg-[#3867F5]")}
          style={{ width: String(percent) + "%" }}
        />
      </div>
      {progress && (
        <div className="mt-2 flex flex-wrap gap-x-4 gap-y-1 text-[11px] text-[#A1A1AA]">
          <span>{text ? "Scene tersimpan" : "Scenes saved"}: {progress.completed_scenes}/{progress.scene_count}</span>
          {progress.active_scene != null && !failed && (
            <span>{text ? "Sedang proses scene" : "Current scene"}: {progress.active_scene}</span>
          )}
        </div>
      )}
      {progress?.scenes && progress.scenes.length > 1 && (
        <div className="mt-2 grid gap-1">
          {progress.scenes.map((scene) => (
            <div key={scene.number} className="flex items-center justify-between gap-2 text-[11px] text-[#A1A1AA]">
              <span>Scene {scene.number} · {scene.duration}s</span>
              <span>
                {stageText(scene.state, lang)}
                {scene.provider_progress != null && !scene.saved && !["failed", "unknown"].includes(scene.state) && (
                  <> · {Math.min(100, Math.max(0, scene.provider_progress))}%</>
                )}
              </span>
            </div>
          ))}
        </div>
      )}
      {progress?.error && (
        <p className="mt-2 break-words rounded border border-red-500/30 bg-red-500/10 px-2 py-1.5 text-[11px] text-red-300">
          {progress.error}
        </p>
      )}
      {error && !failed && (
        <p className="mt-2 break-words text-[11px] text-amber-300">
          {text ? "Koneksi progres terputus sementara. Mencoba lagi otomatis." : "Progress connection temporarily unavailable. Retrying automatically."}
          <span className="sr-only">{error}</span>
        </p>
      )}
      {finalPath && (
        <div className="mt-3 border-t border-[#343541] pt-2">
          <p className="mb-1 font-semibold text-emerald-400">
            {text ? "MP4 sudah tersimpan di project" : "MP4 saved in your project"}
          </p>
          <p className="break-all font-mono text-[11px] text-[#A1A1AA]">{finalPath}</p>
          <button
            className="mt-2 inline-flex items-center gap-1.5 rounded border border-[#343541] px-2 py-1 text-[11px] text-[#8aa8ff] hover:border-[#3867F5] hover:text-white"
            onClick={() => {
              void navigator.clipboard.writeText(finalPath).then(() => setCopied(true)).catch(() => setCopied(false));
            }}
          >
            {copied ? <Check className="h-3 w-3" /> : <Copy className="h-3 w-3" />}
            {copied ? (text ? "Disalin" : "Copied") : (text ? "Salin lokasi MP4" : "Copy MP4 path")}
          </button>
        </div>
      )}
    </div>
  );
}
