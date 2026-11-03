// Native folder picker for the workspace (C1).
//
// Uses the Tauri dialog plugin directly through invoke
// ("plugin:dialog|open") instead of adding the @tauri-apps/plugin-dialog
// npm package — same command, zero new dependency. NOTE: the dialog
// plugin must ALSO be registered on the Rust side
// (.plugin(tauri_plugin_dialog::init()) + tauri-plugin-dialog in
// Cargo.toml + "dialog:default" in capabilities) before this works in
// the built app; until then open() rejects and the caller falls back
// to the manual path input with an explanatory note. See the
// coordinator report — this file is the frontend half only.
import { invoke } from "@tauri-apps/api/core";

export class DialogPluginMissingError extends Error {
  constructor() {
    super("dialog-plugin-missing");
    this.name = "DialogPluginMissingError";
  }
}

/** Open the native folder picker. Returns null when the user cancels. */
export async function pickFolder(currentPath?: string): Promise<string | null> {
  let res: unknown;
  try {
    res = await invoke("plugin:dialog|open", {
      options: {
        directory: true,
        multiple: false,
        title: "Choose a project folder for the agent to work in",
        defaultPath: currentPath || undefined,
      },
    });
  } catch (e) {
    const msg = String(e);
    if (/plugin.*dialog|not found|not registered|unknown command/i.test(msg)) {
      throw new DialogPluginMissingError();
    }
    throw e;
  }
  if (res === null || res === undefined) return null;
  if (typeof res === "string") return res;
  if (Array.isArray(res) && res.length > 0 && typeof res[0] === "string") return res[0];
  return null;
}
