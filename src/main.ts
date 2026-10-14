// iDrop — 앱 셸: 퀵커넥트 + 듀얼 패널 배선. (전송 큐는 M3)
import "./styles.css";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { applyI18n, getLang, t } from "./i18n";
import { confirmBox, initDialogs, promptText } from "./dialogs";
import {
  FsAdapter,
  Panel,
  PanelEntry,
  localAdapterPaths,
  remoteAdapterPaths,
} from "./panel";

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

localPanel.onActivateFiles = () => setStatus(t("panel.transferLater")); // M3
remotePanel.onActivateFiles = () => setStatus(t("panel.transferLater")); // M3

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
  mkdir.addEventListener("click", async () => {
    if (!guardRemote(panel) || !panel.path) return;
    const name = await promptText(t("panel.newFolderTitle"));
    if (!name?.trim()) return;
    try {
      await panel.adapter.mkdir(panel.adapter.join(panel.path, name.trim()));
      await panel.refresh();
    } catch (err) {
      setStatus(String(err));
    }
  });
}
wireHead("local", localPanel);
wireHead("remote", remotePanel);

// F2 / Delete (포커스 패널 대상)
document.addEventListener("keydown", async (e) => {
  if (!guardRemote(active) || !active.path) return;
  if (e.key === "F2") {
    e.preventDefault();
    const sel = active.selectedEntries();
    if (sel.length !== 1) return;
    const name = await promptText(t("panel.renameTitle"), sel[0].name);
    if (!name?.trim() || name === sel[0].name) return;
    try {
      await active.adapter.rename(
        active.adapter.join(active.path, sel[0].name),
        active.adapter.join(active.path, name.trim())
      );
      await active.refresh();
    } catch (err) {
      setStatus(String(err));
    }
  } else if (e.key === "Delete") {
    e.preventDefault();
    const paths = active.selectedPaths();
    if (!paths.length) return;
    if (!(await confirmBox(t("panel.deleteTitle"), t("panel.deleteMsg", { n: paths.length }), true)))
      return;
    try {
      await active.adapter.remove(paths);
      await active.refresh();
    } catch (err) {
      setStatus(String(err));
    }
  }
});

// ---- 퀵커넥트 ----------------------------------------------------------------

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
  qcBtn.disabled = true;
  try {
    const home = await invoke<string>("site_connect", {
      hop: {
        host,
        port: parseInt(qcPort.value, 10) || 22,
        username: qcUser.value.trim(),
        password: qcPass.value,
        keyFile: "",
        useAgent: false,
      },
      encoding: "auto",
    });
    await remotePanel.navigate(home);
  } catch (err) {
    setStatus(String(err));
  } finally {
    qcBtn.disabled = false;
  }
}

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
