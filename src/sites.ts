// 사이트 관리자 — 저장된 SFTP 접속 목록(그룹 트리). iDOT 세션은 CSV/Xshell/PuTTY로
// 가져온다(별개 앱이라 localStorage 직접 접근 불가). 비밀번호는 DPAPI 암호문으로 저장.
import { invoke } from "@tauri-apps/api/core";
import { importFiles, ImportResult } from "./sessionio";
import { confirmBox, promptText } from "./dialogs";
import { t } from "./i18n";

export interface Site {
  id: string;
  name: string;
  group: string;
  host: string;
  port: number;
  username: string;
  password: string; // 메모리는 평문, 저장은 "dpapi:…"
  keyFile: string;
  useAgent: boolean;
  nameEncoding: "auto" | "utf-8" | "euc-kr";
  initialRemoteDir: string;
}

const STORE_KEY = "idrop.sites";
const SECRET_PREFIX = "dpapi:";
const secretCache = new Map<string, string>(); // 평문 → 암호문
let sites: Site[] = [];
let saveChain: Promise<void> = Promise.resolve();

function genId(): string {
  return crypto.randomUUID();
}

function normalize(x: unknown): Site | null {
  if (!x || typeof x !== "object") return null;
  const o = x as Record<string, unknown>;
  if (typeof o.host !== "string" || !o.host) return null;
  return {
    id: typeof o.id === "string" ? o.id : genId(),
    name: typeof o.name === "string" && o.name ? o.name : o.host,
    group: typeof o.group === "string" ? o.group : "",
    host: o.host,
    port: typeof o.port === "number" ? o.port : parseInt(String(o.port), 10) || 22,
    username: typeof o.username === "string" ? o.username : "",
    password: typeof o.password === "string" ? o.password : "",
    keyFile: typeof o.keyFile === "string" ? o.keyFile : "",
    useAgent: o.useAgent === true,
    nameEncoding:
      o.nameEncoding === "utf-8" || o.nameEncoding === "euc-kr" ? o.nameEncoding : "auto",
    initialRemoteDir: typeof o.initialRemoteDir === "string" ? o.initialRemoteDir : "",
  };
}

function load(): Site[] {
  try {
    const raw = localStorage.getItem(STORE_KEY);
    if (!raw) return [];
    const list = JSON.parse(raw);
    if (Array.isArray(list)) return list.map(normalize).filter((s): s is Site => s !== null);
  } catch {
    /* ignore */
  }
  return [];
}

function save() {
  const snapshot = sites.map((s) => ({ ...s }));
  saveChain = saveChain
    .then(async () => {
      const need = [...new Set(snapshot.map((s) => s.password).filter((p) => p && !secretCache.has(p)))];
      if (need.length) {
        try {
          const enc = await invoke<string[]>("secret_protect", { values: need });
          need.forEach((p, i) => secretCache.set(p, enc[i]));
        } catch {
          need.forEach((p) => secretCache.set(p, p)); // 암호화 불가 환경 — 평문 유지
        }
      }
      const stored = snapshot.map((s) => ({
        ...s,
        password: s.password ? secretCache.get(s.password) ?? s.password : "",
      }));
      localStorage.setItem(STORE_KEY, JSON.stringify(stored));
    })
    .catch(() => {});
}

/** 저장된 암호문을 평문으로 풀어 메모리에 둔다. 예전 평문은 암호화해 다시 저장. */
async function revealPasswords() {
  const enc = [...new Set(sites.map((s) => s.password).filter((p) => p.startsWith(SECRET_PREFIX)))];
  if (enc.length) {
    try {
      const plain = await invoke<string[]>("secret_unprotect", { values: enc });
      const map = new Map(enc.map((c, i) => [c, plain[i]]));
      enc.forEach((c, i) => plain[i] && secretCache.set(plain[i], c));
      sites = sites.map((s) =>
        s.password.startsWith(SECRET_PREFIX) ? { ...s, password: map.get(s.password) ?? "" } : s
      );
    } catch {
      return;
    }
  }
  if (sites.some((s) => s.password && !secretCache.has(s.password))) save();
}

export async function initSites(onConnect: (s: Site) => void) {
  sites = load();
  await revealPasswords();

  const modal = document.getElementById("sites-modal") as HTMLElement;
  const listEl = document.getElementById("sites-list") as HTMLElement;
  const open = () => {
    modal.classList.remove("hidden");
    renderList();
  };
  const close = () => modal.classList.add("hidden");
  document.getElementById("sites-open")!.addEventListener("click", open);
  document.getElementById("sites-close")!.addEventListener("click", close);
  modal.addEventListener("click", (e) => {
    if (e.target === modal) close();
  });

  // 폼 요소
  const f = {
    name: document.getElementById("sf-name") as HTMLInputElement,
    group: document.getElementById("sf-group") as HTMLInputElement,
    host: document.getElementById("sf-host") as HTMLInputElement,
    port: document.getElementById("sf-port") as HTMLInputElement,
    user: document.getElementById("sf-user") as HTMLInputElement,
    pass: document.getElementById("sf-pass") as HTMLInputElement,
    enc: document.getElementById("sf-enc") as HTMLSelectElement,
  };
  let selectedId: string | null = null;

  function loadForm(s: Site | null) {
    selectedId = s?.id ?? null;
    f.name.value = s?.name ?? "";
    f.group.value = s?.group ?? "";
    f.host.value = s?.host ?? "";
    f.port.value = String(s?.port ?? 22);
    f.user.value = s?.username ?? "";
    f.pass.value = s?.password ?? "";
    f.enc.value = s?.nameEncoding ?? "auto";
  }

  function renderList() {
    listEl.innerHTML = "";
    const groups = new Map<string, Site[]>();
    for (const s of sites) {
      const g = s.group || "";
      if (!groups.has(g)) groups.set(g, []);
      groups.get(g)!.push(s);
    }
    for (const [g, list] of [...groups].sort((a, b) => a[0].localeCompare(b[0]))) {
      if (g) {
        const gh = document.createElement("div");
        gh.className = "site-group";
        gh.textContent = "📁 " + g;
        listEl.appendChild(gh);
      }
      for (const s of list.sort((a, b) => a.name.localeCompare(b.name))) {
        const row = document.createElement("div");
        row.className = "site-item" + (s.id === selectedId ? " sel" : "");
        row.textContent = s.name;
        const del = document.createElement("button");
        del.className = "site-del";
        del.textContent = "✕";
        del.title = t("sites.delete");
        del.onclick = (e) => {
          e.stopPropagation();
          sites = sites.filter((x) => x.id !== s.id);
          save();
          renderList();
        };
        row.onclick = () => {
          loadForm(s);
          renderList();
        };
        row.ondblclick = () => {
          close();
          onConnect(s);
        };
        row.appendChild(del);
        listEl.appendChild(row);
      }
    }
  }

  function formToSite(): Site | null {
    const host = f.host.value.trim();
    if (!host) {
      f.host.focus();
      return null;
    }
    const existing = selectedId ? sites.find((s) => s.id === selectedId) : undefined;
    return {
      id: existing?.id ?? genId(),
      name: f.name.value.trim() || host,
      group: f.group.value.trim(),
      host,
      port: parseInt(f.port.value, 10) || 22,
      username: f.user.value.trim(),
      password: f.pass.value,
      keyFile: existing?.keyFile ?? "",
      useAgent: existing?.useAgent ?? false,
      nameEncoding: f.enc.value as Site["nameEncoding"],
      initialRemoteDir: existing?.initialRemoteDir ?? "",
    };
  }

  document.getElementById("sf-new")!.addEventListener("click", () => {
    loadForm(null);
    renderList();
  });
  document.getElementById("sf-save")!.addEventListener("click", () => {
    const s = formToSite();
    if (!s) return;
    const i = sites.findIndex((x) => x.id === s.id);
    if (i >= 0) sites[i] = s;
    else sites.push(s);
    selectedId = s.id;
    save();
    renderList();
  });
  document.getElementById("sf-connect")!.addEventListener("click", () => {
    const s = formToSite();
    if (!s) return;
    const i = sites.findIndex((x) => x.id === s.id);
    if (i >= 0) sites[i] = s;
    else {
      sites.push(s);
      selectedId = s.id;
    }
    save();
    close();
    onConnect(s);
  });

  // 가져오기 (CSV/Xshell/PuTTY)
  const fileInput = document.getElementById("sf-import-file") as HTMLInputElement;
  document.getElementById("sf-import")!.addEventListener("click", () => fileInput.click());
  fileInput.addEventListener("change", async () => {
    if (!fileInput.files?.length) return;
    const res = await importFiles([...fileInput.files]);
    await applyImport(res);
    fileInput.value = "";
  });
  document.getElementById("sf-import-putty")!.addEventListener("click", async () => {
    try {
      const list = await invoke<Record<string, string>[]>("import_putty");
      const { puttyToConns } = await import("./sessionio");
      await applyImport(puttyToConns(list));
    } catch (e) {
      alert(String(e));
    }
  });

  async function applyImport(res: ImportResult) {
    // SSH만 (telnet/serial 제외) — sessionio가 이미 걸러주지만 한 번 더
    const ssh = res.conns.filter((c) => c.protocol === "ssh" && c.host);
    if (!ssh.length) {
      alert(t("sites.importNone"));
      return;
    }
    const grp = (await promptText(t("sites.importGroup"), "imported")) ?? "imported";
    for (const c of ssh) {
      sites.push({
        id: genId(),
        name: c.name || c.host,
        group: c.group || grp,
        host: c.host,
        port: c.port || 22,
        username: c.username,
        password: c.password,
        keyFile: c.keyFile ?? "",
        useAgent: false,
        nameEncoding: c.encoding,
        initialRemoteDir: "",
      });
    }
    save();
    renderList();
    await confirmBox(t("sites.importDone"), t("sites.importedN", { n: ssh.length, skipped: res.skipped }));
  }

  loadForm(null);
}
