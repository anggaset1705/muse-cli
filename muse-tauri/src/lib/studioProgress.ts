// Read-only native Muse Studio media progress.
// No artificial timers increment the percentage; it comes from TaskStore.
import type { ToolActionState } from "./types";

export interface StudioSceneProgress {
  number: number;
  duration: number;
  state: string;
  provider_progress: number | null;
  task_id: string | null;
  saved: boolean;
  error: string | null;
}

export interface StudioProgressSnapshot {
  project_id: string;
  name: string;
  state: "waiting" | "queued" | "rendering" | "saving" | "composing" | "complete" | "failed" | "unknown";
  /** Aggregate of actual provider task progress and saved scene files. */
  progress: number;
  completed_scenes: number;
  scene_count: number;
  active_scene: number | null;
  scenes: StudioSceneProgress[];
  final_video_path: string | null;
  error: string | null;
  updated_at: string;
}

const VALID_ID = /^[a-zA-Z0-9_-]{1,48}$/;

/** Only known, successful queue receipts may start a progress monitor. */
export function studioProjectIdFromAction(action: ToolActionState): string | null {
  const receipt = action.result ?? "";
  if (action.tool === "studio_render_project") {
    const match = /^studio_render_started:\s*([a-zA-Z0-9_-]{1,48})\s*$/m.exec(receipt);
    return match && VALID_ID.test(match[1]) ? match[1] : null;
  }
  if (action.tool === "studio_render_image") {
    if (!/^studio_image_queued:\s*[a-zA-Z0-9_-]{1,100}\s*$/m.test(receipt)) return null;
    const match = /^project:\s*([a-zA-Z0-9_-]{1,48})\s*$/m.exec(receipt);
    return match && VALID_ID.test(match[1]) ? match[1] : null;
  }
  return null;
}

export function studioProgressTerminal(status: StudioProgressSnapshot): boolean {
  return status.state === "complete" || status.state === "failed";
}

export function studioProgressPercent(status: StudioProgressSnapshot): number {
  if (!Number.isFinite(status.progress)) return 0;
  // Treat 100 as actual saved final media only, never simply the queue receipt.
  return Math.min(status.state === "complete" ? 100 : 99, Math.max(0, Math.floor(status.progress)));
}
