// iDrop — 앱 셸: 퀵커넥트 + 듀얼 패널 배선. (전송 큐는 M3)
import "./styles.css";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { applyI18n, getLang, t } from "./i18n";
import { getCurrentWebview } from "@tauri-apps/api/webview";
import { confirmBox, initDialogs, promptText } from "./dialogs";
import {
  FsAdapter,
  Panel,
  PanelEntry,
  localAdapterPaths,
  remoteAdapterPaths,
} from "./panel";
import { initQueue } from "./queue";
import { initSites, recordVisited, Site } from "./sites";

applyI18n();
void invoke("set_lang", { lang: getLang() }).catch(() => {});
await initDialogs();

const statusEl = document.getElementById("status") as HTMLElement;
const qcHost = document.getElementById("qc-host") as HTMLInputElement;
const qcPort = document.getElementById("qc-port") as HTMLInputElement;
const qcUser = document.getElementById("qc-user") as HTMLInputElement;
const qcPass = document.getElementById("qc-pass") as HTMLInputElement;
const qcBtn = document.getElementById("qc-connect") as HTMLButtonElement;

export function setStatus(msg: string) {
  statusEl.textContent = msg;
}

let connected = false;

// ---- 어댑터 ------------------------------------------------------------------

interface LocalEntry {
  name: string;
  isDir: boolean;
  size: number;
  mtime: number;
  hidden: boolean;
  readonly: boolean;
}

const localAdapter: FsAdapter = {
  kind: "local",
  async list(path) {
    const [real, entries] = await invoke<[string, LocalEntry[]]>("local_list", { path });
    return [real, entries as PanelEntry[]];
  },
  mkdir: (path) => invoke("local_mkdir", { path }),
  rename: (from, to) => invoke("local_rename", { from, to }),
  remove: (paths) => invoke("local_remove", { paths }),
  ...localAdapterPaths,
};

const remoteAdapter: FsAdapter = {
  kind: "remote",
  async list(path) {
    return await invoke<[string, PanelEntry[]]>("remote_list", { path });
  },
  mkdir: (path) => invoke("remote_mkdir", { path }),
  rename: (from, to) => invoke("remote_rename", { from, to }),
  remove: (paths) => invoke("remote_remove", { paths }),
  ...remoteAdapterPaths,
};

// ---- 패널 구성 ----------------------------------------------------------------

const localPanel = new Panel(
  localAdapter,
  document.getElementById("local-path") as HTMLInputElement,
  document.getElementById("local-list") as HTMLElement
);
const remotePanel = new Panel(
  remoteAdapter,
  document.getElementById("remote-path") as HTMLInputElement,
  document.getElementById("remote-list") as HTMLElement
);

await initQueue(() => {
  // 전송이 하나 완료될 때마다 대상 패널 새로고침 (과하지 않게 디바운스)
  if (connected) void remotePanel.refresh().catch(() => {});
  void localPanel.refresh().catch(() => {});
});

// 전송: 로컬에서 활성화/드롭 → 업로드, 원격에서 → 다운로드.
function transfer(from: Panel, names: string[]) {
  if (!names.length) return;
  if (!connected) {
    setStatus(t("status.disconnected"));
    return;
  }
  const dir = from.adapter.kind === "local" ? "upload" : "download";
  const sources = names.map((n) => from.adapter.join(from.path, n));
  const destDir = dir === "upload" ? remotePanel.path : localPanel.path;
  if (!destDir) return;
  void invoke("queue_add", { dir, sources, destDir }).catch((e) => setStatus(String(e)));
}

// 더블클릭/Enter → 반대편으로 전송
localPanel.onActivateNames = (names) => transfer(localPanel, names);
remotePanel.onActivateNames = (names) => transfer(remotePanel, names);
// 드롭(반대편 패널에 떨어뜨림) → 끌어온 쪽에서 전송
localPanel.onDropNames = (names) => transfer(remotePanel, names); // 원격→로컬 다운로드
remotePanel.onDropNames = (names) => transfer(localPanel, names); // 로컬→원격 업로드

// 탐색기 → 원격 패널 드롭 (Tauri 파일 드롭). 좌표로 원격 패널 위인지 판정.
void getCurrentWebview().onDragDropEvent((e) => {
  if (e.payload.type !== "drop") return;
  if (!connected || !remotePanel.path) return;
  const rect = document.getElementById("panel-remote")!.getBoundingClientRect();
  const { x, y } = e.payload.position;
  if (x < rect.left || x > rect.right || y < rect.top || y > rect.bottom) return;
  void invoke("queue_add", {
    dir: "upload",
    sources: e.payload.paths,
    destDir: remotePanel.path,
  }).catch((err) => setStatus(String(err)));
});

// 어느 패널이 포커스인지 (F2/Delete/새 폴더 대상)
let active: Panel = localPanel;
document.getElementById("panel-local")!.addEventListener("focusin", () => (active = localPanel));
document.getElementById("panel-remote")!.addEventListener("focusin", () => (active = remotePanel));

function guardRemote(p: Panel): boolean {
  return p.adapter.kind !== "remote" || connected;
}

// 드라이브 선택
const drivesSel = document.getElementById("local-drives") as HTMLSelectElement;
async function loadDrives() {
  const drives = await invoke<string[]>("local_drives");
  drivesSel.innerHTML = "";
  for (const d of drives) {
    const o = document.createElement("option");
    o.value = d;
    o.textContent = d;
    drivesSel.appendChild(o);
  }
}
drivesSel.addEventListener("change", () => void localPanel.navigate(drivesSel.value));

// 새로고침·숨김·새 폴더 버튼
function wireHead(prefix: string, panel: Panel) {
  const refresh = document.getElementById(`${prefix}-refresh`) as HTMLButtonElement;
  const hidden = document.getElementById(`${prefix}-hidden`) as HTMLButtonElement;
  const mkdir = document.getElementById(`${prefix}-mkdir`) as HTMLButtonElement;
  refresh.addEventListener("click", () => {
    if (guardRemote(panel)) void panel.refresh();
  });
  hidden.addEventListener("click", () => {
    panel.setShowHidden(!panel.showHidden);
    hidden.classList.toggle("active", panel.showHidden);
  });
  mkdir.addEventListener("click", () => void mkdirIn(panel));
}
wireHead("local", localPanel);
wireHead("remote", remotePanel);

async function mkdirIn(panel: Panel) {
  if (!guardRemote(panel) || !panel.path) return;
  const name = await promptText(t("panel.newFolderTitle"));
  if (!name?.trim()) return;
  try {
    await panel.adapter.mkdir(panel.adapter.join(panel.path, name.trim()));
    await panel.refresh();
  } catch (err) {
    setStatus(String(err));
  }
}

async function renameIn(panel: Panel) {
  if (!guardRemote(panel) || !panel.path) return;
  const sel = panel.selectedEntries();
  if (sel.length !== 1) return;
  const name = await promptText(t("panel.renameTitle"), sel[0].name);
  if (!name?.trim() || name === sel[0].name) return;
  try {
    await panel.adapter.rename(
      panel.adapter.join(panel.path, sel[0].name),
      panel.adapter.join(panel.path, name.trim())
    );
    await panel.refresh();
  } catch (err) {
    setStatus(String(err));
  }
}

async function deleteIn(panel: Panel) {
  if (!guardRemote(panel) || !panel.path) return;
  const paths = panel.selectedPaths();
  if (!paths.length) return;
  if (!(await confirmBox(t("panel.deleteTitle"), t("panel.deleteMsg", { n: paths.length }), true)))
    return;
  try {
    await panel.adapter.remove(paths);
    await panel.refresh();
  } catch (err) {
    setStatus(String(err));
  }
}

// F2 / Delete (포커스 패널 대상)
document.addEventListener("keydown", (e) => {
  if (e.key === "F2") {
    e.preventDefault();
    void renameIn(active);
  } else if (e.key === "Delete") {
    e.preventDefault();
    void deleteIn(active);
  }
});

// ---- 우클릭 메뉴 --------------------------------------------------------------

let ctxEl: HTMLElement | null = null;

function closeMenu() {
  ctxEl?.remove();
  ctxEl = null;
}

document.addEventListener("click", closeMenu);
document.addEventListener("keydown", (e) => {
  if (e.key === "Escape") closeMenu();
});

function showMenu(x: number, y: number, items: { label: string; action: () => void }[]) {
  closeMenu();
  const menu = document.createElement("div");
  menu.className = "ctxmenu";
  for (const it of items) {
    const row = document.createElement("div");
    row.className = "ctxmenu-item";
    row.textContent = it.label;
    row.addEventListener("click", (e) => {
      e.stopPropagation();
      closeMenu();
      it.action();
    });
    menu.appendChild(row);
  }
  document.body.appendChild(menu);
  // 화면 밖으로 나가지 않게
  const r = menu.getBoundingClientRect();
  menu.style.left = `${Math.min(x, window.innerWidth - r.width - 4)}px`;
  menu.style.top = `${Math.min(y, window.innerHeight - r.height - 4)}px`;
  ctxEl = menu;
}

function panelMenu(panel: Panel, x: number, y: number, onRow: boolean) {
  const isLocal = panel.adapter.kind === "local";
  const items: { label: string; action: () => void }[] = [];
  if (onRow) {
    items.push({
      label: isLocal ? t("menu.upload") : t("menu.download"),
      action: () => transfer(panel, [...panel.selected]),
    });
    items.push({ label: t("menu.rename"), action: () => void renameIn(panel) });
    items.push({ label: t("menu.delete"), action: () => void deleteIn(panel) });
  }
  items.push({ label: t("panel.newFolder"), action: () => void mkdirIn(panel) });
  items.push({ label: t("menu.refresh"), action: () => void panel.refresh() });
  showMenu(x, y, items);
}

localPanel.onContext = (x, y, onRow) => panelMenu(localPanel, x, y, onRow);
remotePanel.onContext = (x, y, onRow) => {
  if (!connected) return;
  panelMenu(remotePanel, x, y, onRow);
};

// ---- 접속 (퀵커넥트 / 사이트) -------------------------------------------------

// 지금 접속된 사이트 id (퀵커넥트는 null) — 방문 위치 기억 대상
let activeSiteId: string | null = null;

async function connect(opts: {
  host: string;
  port: number;
  username: string;
  password: string;
  keyFile: string;
  useAgent: boolean;
  encoding: "auto" | "utf-8" | "euc-kr";
  siteId?: string | null;
  remoteDir?: string; // 기억된/초기 원격 경로 (없으면 홈)
  localDir?: string; // 기억된 로컬 경로 (없으면 현재 유지)
}) {
  qcBtn.disabled = true;
  activeSiteId = opts.siteId ?? null;
  try {
    const home = await invoke<string>("site_connect", {
      hop: {
        host: opts.host,
        port: opts.port,
        username: opts.username,
        password: opts.password,
        keyFile: opts.keyFile,
        useAgent: opts.useAgent,
      },
      encoding: opts.encoding,
    });
    // 로컬·원격을 한 번에 기억된 위치로 이동 (원격 실패 시 홈으로 폴백)
    if (opts.localDir) await localPanel.navigate(opts.localDir).catch(() => {});
    try {
      await remotePanel.navigate(opts.remoteDir || home);
    } catch {
      await remotePanel.navigate(home).catch(() => {});
    }
  } catch (err) {
    setStatus(String(err));
  } finally {
    qcBtn.disabled = false;
  }
}

async function quickConnect() {
  if (connected) {
    await invoke("site_disconnect").catch(() => {});
    return;
  }
  const host = qcHost.value.trim();
  if (!host) {
    qcHost.focus();
    return;
  }
  await connect({
    host,
    port: parseInt(qcPort.value, 10) || 22,
    username: qcUser.value.trim(),
    password: qcPass.value,
    keyFile: "",
    useAgent: false,
    encoding: "auto",
    siteId: null,
  });
}

const sitesCtl = await initSites((s: Site) => {
  void connect({
    host: s.host,
    port: s.port,
    username: s.username,
    password: s.password,
    keyFile: s.keyFile,
    useAgent: s.useAgent,
    encoding: s.nameEncoding,
    siteId: s.id,
    remoteDir: s.lastRemoteDir || s.initialRemoteDir || undefined,
    localDir: s.lastLocalDir || undefined,
  });
});

// 접속된 사이트가 있으면 패널 이동 시마다 마지막 위치를 기억한다
localPanel.onNavigated = (path) => {
  if (activeSiteId) recordVisited(activeSiteId, "local", path);
};
remotePanel.onNavigated = (path) => {
  if (connected && activeSiteId) recordVisited(activeSiteId, "remote", path);
};

qcBtn.addEventListener("click", () => void quickConnect());
for (const el of [qcHost, qcPort, qcUser, qcPass]) {
  el.addEventListener("keydown", (e) => {
    if (e.key === "Enter") void quickConnect();
  });
}

// ---- 접속 상태 ----------------------------------------------------------------

await listen<{ state: string; host: string; home?: string; error?: string }>(
  "site-state",
  (e) => {
    const p = e.payload;
    if (p.state === "connecting") {
      setStatus(t("status.connecting", { host: p.host }));
    } else if (p.state === "connected") {
      connected = true;
      qcBtn.textContent = t("top.disconnect");
      setStatus(t("status.connected", { host: p.host }));
    } else {
      connected = false;
      activeSiteId = null;
      qcBtn.textContent = t("top.connect");
      setStatus(p.error ? p.error : t("status.disconnected"));
    }
  }
);

// ---- 시작 ---------------------------------------------------------------------

await loadDrives();
const home = await invoke<string>("local_home").catch(() => "");
if (home) {
  await localPanel.navigate(home).catch(() => {});
  const drive = home.slice(0, 3);
  for (const o of drivesSel.options) if (o.value.toUpperCase() === drive.toUpperCase()) o.selected = true;
}

// 최초 기동: 사이트 관리자를 띄워 바로 접속을 고를 수 있게 한다
if (sitesCtl.hasSites()) sitesCtl.open();
