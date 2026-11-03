// Unit test untuk guard riwayat percakapan (4 Nov 2026) + default driver.
// Jalankan:  node scripts/test-conversation-guard.mjs
// Memakai kode store.ts / types.ts ASLI (via jiti dari node_modules) dengan
// fake localStorage, mensimulasikan urutan efek mount App.tsx secara setia:
//   initializer useState -> efek B6 -> efek save (mount).
// Keluar 0 bila semua lulus, 1 bila ada yang gagal.
import { createJiti } from "../node_modules/jiti/lib/jiti.mjs";

const repoRoot = new URL("..", import.meta.url).pathname;
const jiti = createJiti(import.meta.url);
const store = jiti(`${repoRoot}src/lib/store.ts`);
const { DEFAULT_SETTINGS } = jiti(`${repoRoot}src/lib/types.ts`);

const FOX = "C:\\Users\\B-STORE\\Desktop\\FOX";
const FOX_KEY = "muse.conversations.v1::c%3A%5Cusers%5Cb-store%5Cdesktop%5Cfox";

function makeStorage(seed = {}) {
  const map = new Map(Object.entries(seed));
  return {
    map,
    getItem(k) { return map.has(k) ? map.get(k) : null; },
    setItem(k, v) { map.set(k, String(v)); },
    removeItem(k) { map.delete(k); },
  };
}

const conv1 = {
  id: "conv-1", title: "halo apa kabar ?", createdAt: 1, updatedAt: 1,
  pinned: false,
  messages: [
    { id: "m1", role: "user", content: "halo apa kabar ?", createdAt: 1 },
    { id: "m2", role: "assistant", content: "[mock:muse-chat] You said: halo apa kabar ?", createdAt: 2 },
  ],
};
const conv2 = { ...conv1, id: "conv-2", title: "percakapan kedua" };
const settingsFox = JSON.stringify({
  uiLanguage: "id", followQuestionLanguage: true, theme: "dark", turboMode: "ask",
  backendBaseUrl: "http://127.0.0.1:18610", apiKey: "", model: "muse-chat",
  workspacePath: FOX, shell: "", driver: "browser",
});

// Simulasi mount App.tsx: initializer + efek B6 (mount, no-op) + efek save.
function simulateMount(storage) {
  globalThis.localStorage = storage;
  const settings = store.loadSettings();
  const conversations = store.loadConversations(settings.workspacePath);
  store.saveConversations(conversations, settings.workspacePath);
  return { settings, conversations };
}

let pass = 0, fail = 0;
function check(name, cond, extra = "") {
  if (cond) { pass++; console.log(`PASS  ${name}`); }
  else { fail++; console.log(`FAIL  ${name} ${extra}`); }
}

// T0: default driver harus browser (Perbaikan 1).
check("T0 DEFAULT_SETTINGS.driver = browser", DEFAULT_SETTINGS.driver === "browser", `dapat: ${DEFAULT_SETTINGS.driver}`);

// T1: mount normal — riwayat FOX utuh.
{
  const storage = makeStorage({ "muse.settings.v1": settingsFox, [FOX_KEY]: JSON.stringify([conv1]) });
  simulateMount(storage);
  const after = JSON.parse(storage.getItem(FOX_KEY));
  check("T1 mount normal: riwayat FOX utuh", after.length === 1 && after[0].title === "halo apa kabar ?");
}

// T2: load awal gagal total (getItem null sementara) -> state [] -> efek save
// mount. Guard HARUS menolak menimpa daftar tersimpan; load ulang (efek
// recovery App.tsx) harus mengembalikan daftar.
{
  const storage = makeStorage({ "muse.settings.v1": settingsFox, [FOX_KEY]: JSON.stringify([conv1]) });
  globalThis.localStorage = storage;
  const realGet = storage.getItem.bind(storage);
  storage.getItem = (k) => (k === FOX_KEY ? null : realGet(k)); // read gagal
  const settings = store.loadSettings();
  const conversations = store.loadConversations(settings.workspacePath); // []
  const loadEmpty = conversations.length === 0;
  storage.getItem = realGet; // storage pulih sebelum efek save jalan
  store.saveConversations(conversations, settings.workspacePath); // efek save mount
  const after = JSON.parse(storage.getItem(FOX_KEY));
  check("T2a load awal kosong (pemicu)", loadEmpty);
  check("T2b guard: [] TIDAK menimpa riwayat tersimpan", after.length === 1 && after[0].title === "halo apa kabar ?");
  const recovered = store.loadConversations(settings.workspacePath);
  check("T2c recovery: load ulang mengembalikan riwayat", recovered.length === 1);
}

// T3: ganti workspace FOX -> Muse Workspace: daftar lama terselamatkan.
{
  const storage = makeStorage({ "muse.settings.v1": settingsFox, [FOX_KEY]: JSON.stringify([conv1]) });
  const { settings, conversations } = simulateMount(storage);
  const next = "C:\\Users\\B-STORE\\Muse Workspace";
  store.saveConversations(conversations, settings.workspacePath); // efek B6: save lama
  const list2 = store.loadConversations(next); // efek B6: load baru
  store.saveConversations(list2, next); // efek save sesudah setConversations([])
  const afterFox = JSON.parse(storage.getItem(FOX_KEY));
  check("T3 ganti workspace: daftar FOX lama terselamatkan", afterFox.length === 1);
}

// T4: hapus satu-satunya percakapan (eksplisit, flag) -> [] BOLEH tersimpan.
{
  const storage = makeStorage({ "muse.settings.v1": settingsFox, [FOX_KEY]: JSON.stringify([conv1]) });
  const { settings } = simulateMount(storage);
  store.saveConversations([], settings.workspacePath, { allowEmptyOverwrite: true });
  const after = JSON.parse(storage.getItem(FOX_KEY));
  check("T4 hapus terakhir (eksplisit): [] tersimpan", Array.isArray(after) && after.length === 0);
}

// T5: hapus satu dari dua percakapan (tanpa flag) -> sisa satu tersimpan.
{
  const storage = makeStorage({ "muse.settings.v1": settingsFox, [FOX_KEY]: JSON.stringify([conv1, conv2]) });
  const { settings } = simulateMount(storage);
  store.saveConversations([conv2], settings.workspacePath);
  const after = JSON.parse(storage.getItem(FOX_KEY));
  check("T5 hapus satu dari dua: sisa tersimpan", after.length === 1 && after[0].id === "conv-2");
}

// T6: [] di atas [] yang sudah tersimpan -> tetap [] (idempotent, tidak error).
{
  const storage = makeStorage({ "muse.settings.v1": settingsFox, [FOX_KEY]: "[]" });
  globalThis.localStorage = storage;
  store.saveConversations([], FOX);
  check("T6 [] di atas []: tetap []", storage.getItem(FOX_KEY) === "[]");
}

// T7: nilai tersimpan korup (bukan JSON array) -> [] tanpa flag DITOLAK.
{
  const storage = makeStorage({ "muse.settings.v1": settingsFox, [FOX_KEY]: "{bukan-json" });
  globalThis.localStorage = storage;
  store.saveConversations([], FOX);
  check("T7 nilai korup: [] tanpa flag ditolak", storage.getItem(FOX_KEY) === "{bukan-json");
}

console.log(`\n${pass} lulus, ${fail} gagal`);
process.exit(fail ? 1 : 0);
