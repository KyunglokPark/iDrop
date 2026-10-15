// 사이트 관리자 — iDOT 접속 관리와 같은 그룹 트리 UI.
//  - 그룹 접기/펼치기(caret), 호버 버튼(✎ 이름변경 · ⧉ 복사 · ✕ 삭제), 더블클릭 접속
//  - 포인터 드래그로 순서 변경·그룹 이동 (WebView2에서 HTML5 DnD가 안 되므로 직접 구현)
//  - 가져오기: iDOT(localStorage 직접 읽기 — 비밀번호까지), 파일(iDrop/iDOT CSV·Xshell)
// 비밀번호는 DPAPI 암호문("dpapi:…")으로 저장한다. iDOT와 엔트로피가 같아
// iDOT에서 가져온 암호문도 그대로 풀린다.
import { invoke } from "@tauri-apps/api/core";
import { importFiles, ImportedConn, ImportResult } from "./sessionio";
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
const GROUPS_KEY = "idrop.groups";
const COLLAPSED_KEY = "idrop.collapsedGroups";
const SECRET_PREFIX = "dpapi:";

const secretCache = new Map<string, string>(); // 평문 → 암호문
let sites: Site[] = [];
let groups: string[] = [];
const collapsed = new Set<string>();
let selectedId: string | null = null;
let selectedGroup: string | null = null;
let saveChain: Promise<void> = Promise.resolve();

function genId(): string {
  return crypto.randomUUID();
}

// ---- 저장/복원 ---------------------------------------------------------------

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

function loadAll() {
  try {
    const raw = localStorage.getItem(STORE_KEY);
    const list = raw ? JSON.parse(raw) : [];
    if (Array.isArray(list)) sites = list.map(normalize).filter((s): s is Site => s !== null);
  } catch {
    sites = [];
  }
  try {
    const raw = localStorage.getItem(GROUPS_KEY);
    const arr = raw ? JSON.parse(raw) : [];
    if (Array.isArray(arr)) groups = arr.filter((x): x is string => typeof x === "string");
  } catch {
    groups = [];
  }
  try {
    const raw = localStorage.getItem(COLLAPSED_KEY);
    const arr = raw ? JSON.parse(raw) : [];
    if (Array.isArray(arr)) for (const g of arr) if (typeof g === "string") collapsed.add(g);
  } catch {
    /* ignore */
  }
}

function saveGroups() {
  localStorage.setItem(GROUPS_KEY, JSON.stringify(groups));
}

function saveCollapsed() {
  localStorage.setItem(COLLAPSED_KEY, JSON.stringify([...collapsed]));
}

function save() {
  const snapshot = sites.map((s) => ({ ...s }));
  saveChain = saveChain
    .then(async () => {
      const need = [...new Set(snapshot.map((s) => s.password).filter((p) => p && !p.startsWith(SECRET_PREFIX) && !secretCache.has(p)))];
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
        password: s.password && !s.password.startsWith(SECRET_PREFIX)
          ? secretCache.get(s.password) ?? s.password
          : s.password,
      }));
      localStorage.setItem(STORE_KEY, JSON.stringify(stored));
    })
    .catch(() => {});
}

/** 암호문을 평문으로 풀어 메모리에 둔다 (가져오기 후에도 재호출). */
async function revealPasswords() {
  const enc = [...new Set(sites.map((s) => s.password).filter((p) => p.startsWith(SECRET_PREFIX)))];
  if (!enc.length) return;
  try {
    const plain = await invoke<string[]>("secret_unprotect", { values: enc });
    const map = new Map(enc.map((c, i) => [c, plain[i]]));
    enc.forEach((c, i) => plain[i] && secretCache.set(plain[i], c));
    sites = sites.map((s) =>
      s.password.startsWith(SECRET_PREFIX) ? { ...s, password: map.get(s.password) || "" } : s
    );
  } catch {
    /* Tauri 밖 */
  }
}

// ---- 초기화 -------------------------------------------------------------------

export async function initSites(onConnect: (s: Site) => void) {
  loadAll();
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

  // ---- 폼 ----
  const f = {
    name: document.getElementById("sf-name") as HTMLInputElement,
    group: document.getElementById("sf-group") as HTMLInputElement,
    host: document.getElementById("sf-host") as HTMLInputElement,
    port: document.getElementById("sf-port") as HTMLInputElement,
    user: document.getElementById("sf-user") as HTMLInputElement,
    pass: document.getElementById("sf-pass") as HTMLInputElement,
    enc: document.getElementById("sf-enc") as HTMLSelectElement,
  };

  function loadForm(s: Site | null) {
    selectedId = s?.id ?? null;
    f.name.value = s?.name ?? "";
    f.group.value = s?.group ?? selectedGroup ?? "";
    f.host.value = s?.host ?? "";
    f.port.value = String(s?.port ?? 22);
    f.user.value = s?.username ?? "";
    f.pass.value = s?.password ?? "";
    f.enc.value = s?.nameEncoding ?? "auto";
  }

  function clearForm() {
    loadForm(null);
    renderList();
  }

  function selectSite(s: Site) {
    selectedGroup = null;
    loadForm(s);
    renderList();
  }

  function selectGroup(g: string) {
    const isOpen = selectedGroup === g && !collapsed.has(g);
    selectedId = null;
    if (isOpen) {
      selectedGroup = null;
      collapsed.add(g);
    } else {
      selectedGroup = g;
      collapsed.delete(g);
    }
    saveCollapsed();
    loadForm(null);
    renderList();
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

  function upsert(): Site | null {
    const s = formToSite();
    if (!s) return null;
    const i = sites.findIndex((x) => x.id === s.id);
    if (i >= 0) sites[i] = s;
    else sites.push(s);
    if (s.group && !groups.includes(s.group)) {
      groups.push(s.group);
      saveGroups();
    }
    selectedId = s.id;
    save();
    renderList();
    return s;
  }

  document.getElementById("sf-save")!.addEventListener("click", () => void upsert());
  document.getElementById("sf-connect")!.addEventListener("click", () => {
    const s = upsert();
    if (!s) return;
    close();
    onConnect(s);
  });

  // ---- 리스트 동작 ----

  function deleteSite(id: string) {
    sites = sites.filter((s) => s.id !== id);
    save();
    if (selectedId === id) clearForm();
    else renderList();
  }

  function deleteGroup(g: string) {
    for (const s of sites) if (s.group === g) s.group = "";
    groups = groups.filter((x) => x !== g);
    collapsed.delete(g);
    saveGroups();
    saveCollapsed();
    save();
    if (selectedGroup === g) selectedGroup = null;
    renderList();
  }

  function copyName(base: string, taken: Set<string>): string {
    for (let n = 1; ; n++) {
      const name = `${base}(${n})`;
      if (!taken.has(name)) return name;
    }
  }

  function copySite(id: string) {
    const i = sites.findIndex((s) => s.id === id);
    if (i < 0) return;
    const src = sites[i];
    const copy: Site = { ...src, id: genId(), name: copyName(src.name, new Set(sites.map((s) => s.name))) };
    sites.splice(i + 1, 0, copy);
    save();
    selectSite(copy);
  }

  function copyGroup(g: string) {
    const taken = new Set([...groups, ...sites.map((s) => s.group).filter(Boolean)]);
    const name = copyName(g, taken);
    if (!groups.includes(g)) groups.push(g);
    groups.splice(groups.indexOf(g) + 1, 0, name);
    let last = -1;
    sites.forEach((s, i) => {
      if (s.group === g) last = i;
    });
    const copies = sites.filter((s) => s.group === g).map((s) => ({ ...s, id: genId(), group: name }));
    sites.splice(last + 1, 0, ...copies);
    saveGroups();
    collapsed.delete(name);
    saveCollapsed();
    save();
    selectedGroup = name;
    selectedId = null;
    loadForm(null);
    renderList();
  }

  function renameGroup(oldName: string, newName: string): boolean {
    const n = newName.trim();
    if (!n || n === oldName) return true;
    if (groups.includes(n) || sites.some((s) => s.group === n)) return false;
    if (groups.includes(oldName)) groups[groups.indexOf(oldName)] = n;
    else groups.push(n);
    for (const s of sites) if (s.group === oldName) s.group = n;
    if (collapsed.delete(oldName)) collapsed.add(n);
    if (selectedGroup === oldName) selectedGroup = n;
    saveGroups();
    saveCollapsed();
    save();
    return true;
  }

  /** 그룹 머리의 이름을 입력칸으로 바꿔 바로 수정 (iDOT와 동일) */
  function startRenameGroup(label: HTMLElement, g: string) {
    const input = document.createElement("input");
    input.className = "group-rename";
    input.value = g;
    input.spellcheck = false;
    label.replaceWith(input);
    input.focus();
    input.select();
    let done = false;
    const finish = (saveIt: boolean) => {
      if (done) return;
      done = true;
      if (saveIt && !renameGroup(g, input.value)) {
        done = false;
        input.focus();
        input.select();
        return;
      }
      renderList();
    };
    input.addEventListener("keydown", (e) => {
      e.stopPropagation();
      if (e.key === "Enter") {
        e.preventDefault();
        finish(true);
      } else if (e.key === "Escape") {
        e.preventDefault();
        finish(false);
      }
    });
    input.addEventListener("blur", () => finish(true));
    for (const ev of ["click", "dblclick", "pointerdown"])
      input.addEventListener(ev, (e) => e.stopPropagation());
  }

  function moveSite(id: string, group: string) {
    const s = sites.find((x) => x.id === id);
    if (!s || s.group === group) return;
    s.group = group;
    save();
    if (group) {
      if (!groups.includes(group)) {
        groups.push(group);
        saveGroups();
      }
      collapsed.delete(group);
      saveCollapsed();
    }
    renderList();
  }

  function moveSiteNear(dragId: string, targetId: string, before: boolean) {
    if (dragId === targetId) return;
    const from = sites.findIndex((s) => s.id === dragId);
    if (from < 0) return;
    const [item] = sites.splice(from, 1);
    const ti = sites.findIndex((s) => s.id === targetId);
    if (ti < 0) {
      sites.splice(from, 0, item);
      return;
    }
    item.group = sites[ti].group;
    sites.splice(before ? ti : ti + 1, 0, item);
    save();
    renderList();
  }

  function moveGroupNear(dragG: string, targetG: string, before: boolean) {
    if (dragG === targetG) return;
    for (const g of [dragG, targetG]) if (!groups.includes(g)) groups.push(g);
    groups.splice(groups.indexOf(dragG), 1);
    const ti = groups.indexOf(targetG);
    groups.splice(before ? ti : ti + 1, 0, dragG);
    saveGroups();
    renderList();
  }

  // ---- 포인터 드래그 (iDOT 접속 관리와 동일) ----

  type DragSrc = { kind: "srv"; id: string } | { kind: "grp"; g: string };
  interface ListDrag {
    src: DragSrc;
    el: HTMLElement;
    x: number;
    y: number;
    pointerId: number;
    active: boolean;
  }
  let listDrag: ListDrag | null = null;
  let dragJustEnded = false;

  type DropMode = "before" | "after" | "into" | "root";

  function dropAt(d: ListDrag, x: number, y: number): { el: HTMLElement | null; mode: DropMode | null } {
    const hit = document.elementFromPoint(x, y) as HTMLElement | null;
    if (!hit || !listEl.contains(hit)) return { el: null, mode: null };
    const li = hit.closest<HTMLElement>("li.conn, li.group-header");
    if (!li || li === d.el) {
      return { el: null, mode: !li && d.src.kind === "srv" ? "root" : null };
    }
    const before = y < li.getBoundingClientRect().top + li.offsetHeight / 2;
    const isGroup = li.classList.contains("group-header");
    if (d.src.kind === "srv") return { el: li, mode: isGroup ? "into" : before ? "before" : "after" };
    return isGroup ? { el: li, mode: before ? "before" : "after" } : { el: null, mode: null };
  }

  function clearDropMarks() {
    for (const el of Array.from(listEl.querySelectorAll(".drop-before, .drop-after, .drop-target")))
      el.classList.remove("drop-before", "drop-after", "drop-target");
    listEl.classList.remove("drop-root");
  }

  function attachListDrag(el: HTMLElement, src: DragSrc) {
    el.addEventListener("pointerdown", (e) => {
      if (e.button !== 0 || (e.target as HTMLElement).closest("button, .caret, input")) return;
      e.preventDefault();
      listDrag = { src, el, x: e.clientX, y: e.clientY, pointerId: e.pointerId, active: false };
    });
  }

  listEl.addEventListener("dragstart", (e) => e.preventDefault());

  function endDrag(d: ListDrag) {
    d.el.classList.remove("dragging");
    document.body.classList.remove("list-dragging");
    clearDropMarks();
    dragJustEnded = true;
    window.setTimeout(() => (dragJustEnded = false), 0);
  }

  window.addEventListener("pointermove", (e) => {
    const d = listDrag;
    if (!d || e.pointerId !== d.pointerId) return;
    if (!d.active) {
      if (Math.abs(e.clientX - d.x) + Math.abs(e.clientY - d.y) < 6) return;
      d.active = true;
      d.el.classList.add("dragging");
      document.body.classList.add("list-dragging");
    }
    const r = listEl.getBoundingClientRect();
    if (e.clientY < r.top + 24) listEl.scrollTop -= 10;
    else if (e.clientY > r.bottom - 24) listEl.scrollTop += 10;
    clearDropMarks();
    const { el, mode } = dropAt(d, e.clientX, e.clientY);
    if (mode === "root") listEl.classList.add("drop-root");
    else if (el && mode) el.classList.add(mode === "into" ? "drop-target" : `drop-${mode}`);
  });

  window.addEventListener("pointerup", (e) => {
    const d = listDrag;
    if (!d || e.pointerId !== d.pointerId) return;
    listDrag = null;
    if (!d.active) return;
    const { el, mode } = dropAt(d, e.clientX, e.clientY);
    endDrag(d);
    if (d.src.kind === "srv") {
      if (mode === "root") moveSite(d.src.id, "");
      else if (el && mode === "into") moveSite(d.src.id, el.dataset.group ?? "");
      else if (el && (mode === "before" || mode === "after"))
        moveSiteNear(d.src.id, el.dataset.id ?? "", mode === "before");
    } else if (el && (mode === "before" || mode === "after")) {
      moveGroupNear(d.src.g, el.dataset.group ?? "", mode === "before");
    }
  });

  window.addEventListener("pointercancel", (e) => {
    const d = listDrag;
    if (!d || e.pointerId !== d.pointerId) return;
    listDrag = null;
    if (d.active) endDrag(d);
  });

  // ---- 리스트 렌더링 (iDOT connItem/renderList 이식) ----

  function siteItem(s: Site, indented: boolean): HTMLLIElement {
    const li = document.createElement("li");
    li.dataset.id = s.id;
    li.className = "conn" + (indented ? " indent" : "");
    li.title = s.name;

    const name = document.createElement("span");
    name.className = "s-name";
    name.textContent = s.name;

    const sub = document.createElement("span");
    sub.className = "s-sub";
    sub.textContent = `sftp  ${s.username ? s.username + "@" : ""}${s.host}:${s.port}`;

    const cc = document.createElement("button");
    cc.className = "conn-copy";
    cc.textContent = "⧉";
    cc.title = t("sites.copySite");
    cc.addEventListener("click", (e) => {
      e.stopPropagation();
      copySite(s.id);
    });

    const cx = document.createElement("button");
    cx.className = "conn-x";
    cx.textContent = "✕";
    cx.title = t("sites.delete");
    cx.addEventListener("click", async (e) => {
      e.stopPropagation();
      if (await confirmBox(t("sites.delete"), t("sites.deleteSiteConfirm", { name: s.name }), true))
        deleteSite(s.id);
    });

    li.append(name, sub, cc, cx);
    if (s.id === selectedId) li.classList.add("active");
    li.addEventListener("click", () => {
      if (!dragJustEnded) selectSite(s);
    });
    li.addEventListener("dblclick", () => {
      close();
      onConnect(s);
    });
    attachListDrag(li, { kind: "srv", id: s.id });
    return li;
  }

  function renderList() {
    listEl.innerHTML = "";
    const hasAnyGroup = groups.length > 0 || sites.some((s) => s.group);
    if (sites.length === 0 && !hasAnyGroup) {
      const li = document.createElement("li");
      li.className = "empty";
      li.textContent = t("sites.empty");
      listEl.appendChild(li);
      return;
    }

    for (const s of sites) if (!s.group) listEl.appendChild(siteItem(s, false));

    const order = [...groups];
    for (const s of sites) if (s.group && !order.includes(s.group)) order.push(s.group);

    for (const g of order) {
      const members = sites.filter((s) => s.group === g);
      const isCollapsed = collapsed.has(g);

      const header = document.createElement("li");
      const groupActive = g === selectedGroup && selectedId === null;
      header.className = "group-header" + (groupActive ? " active" : "");
      header.dataset.group = g;
      header.title = g;

      const caret = document.createElement("span");
      caret.className = "caret";
      caret.textContent = isCollapsed ? "▸" : "▾";
      caret.addEventListener("click", (e) => {
        e.stopPropagation();
        if (collapsed.has(g)) collapsed.delete(g);
        else collapsed.add(g);
        saveCollapsed();
        renderList();
      });

      const label = document.createElement("span");
      label.className = "group-name";
      label.textContent = g;

      const count = document.createElement("span");
      count.className = "group-count";
      count.textContent = String(members.length);

      const ge = document.createElement("button");
      ge.className = "group-edit";
      ge.textContent = "✎";
      ge.title = t("sites.renameGroup");
      ge.addEventListener("click", (e) => {
        e.stopPropagation();
        startRenameGroup(label, g);
      });

      const gc = document.createElement("button");
      gc.className = "group-copy";
      gc.textContent = "⧉";
      gc.title = t("sites.copyGroup");
      gc.addEventListener("click", (e) => {
        e.stopPropagation();
        copyGroup(g);
      });

      const gx = document.createElement("button");
      gx.className = "group-x";
      gx.textContent = "✕";
      gx.title = t("sites.deleteGroup");
      gx.addEventListener("click", async (e) => {
        e.stopPropagation();
        if (await confirmBox(t("sites.deleteGroup"), t("sites.deleteGroupConfirm", { name: g, n: members.length }), true))
          deleteGroup(g);
      });

      header.append(caret, label, count, ge, gc, gx);
      header.addEventListener("click", () => {
        if (!dragJustEnded) selectGroup(g);
      });
      attachListDrag(header, { kind: "grp", g });
      listEl.appendChild(header);

      if (!isCollapsed) for (const s of members) listEl.appendChild(siteItem(s, true));
    }
  }

  // ---- 새 그룹 / 새 사이트 ----

  document.getElementById("sf-new-group")!.addEventListener("click", async () => {
    const name = (await promptText(t("sites.newGroupTitle")))?.trim();
    if (!name) return;
    if (!groups.includes(name)) {
      groups.push(name);
      saveGroups();
    }
    collapsed.delete(name);
    saveCollapsed();
    selectedGroup = name;
    selectedId = null;
    loadForm(null);
    renderList();
  });

  document.getElementById("sf-new")!.addEventListener("click", () => {
    selectedId = null;
    loadForm(null);
    renderList();
    f.name.focus();
  });

  // ---- 가져오기 ----

  /** 중복(이름+호스트+포트+계정 동일) 건너뛰고 추가. 추가된 개수를 돌려준다. */
  function addImported(conns: { name: string; group: string; host: string; port: number; username: string; password: string; keyFile?: string; encoding: "auto" | "utf-8" | "euc-kr" }[]): number {
    const key = (s: { name: string; host: string; port: number; username: string }) =>
      `${s.name} ${s.host} ${s.port} ${s.username}`;
    const existing = new Set(sites.map(key));
    let added = 0;
    for (const c of conns) {
      if (existing.has(key(c))) continue;
      sites.push({
        id: genId(),
        name: c.name || c.host,
        group: c.group || "",
        host: c.host,
        port: c.port || 22,
        username: c.username,
        password: c.password,
        keyFile: c.keyFile ?? "",
        useAgent: false,
        nameEncoding: c.encoding,
        initialRemoteDir: "",
      });
      if (c.group && !groups.includes(c.group)) groups.push(c.group);
      added++;
    }
    return added;
  }

  async function finishImport(added: number, skippedUnsupported: number) {
    saveGroups();
    save();
    await revealPasswords();
    renderList();
    await confirmBox(t("sites.importDone"), t("sites.importedN", { n: added, skipped: skippedUnsupported }));
  }

  // iDOT에서 가져오기 — localStorage(LevelDB)를 백엔드가 직접 읽는다
  document.getElementById("sf-import-idot")!.addEventListener("click", async () => {
    try {
      const r = await invoke<{ servers: string; groups: string }>("import_idot");
      const conns = (JSON.parse(r.servers) as Record<string, unknown>[])
        .filter((c) => c.protocol === "ssh" && typeof c.host === "string" && c.host)
        .map((c) => ({
          name: String(c.name ?? c.host),
          group: String(c.group ?? ""),
          host: String(c.host),
          port: typeof c.port === "number" ? c.port : parseInt(String(c.port), 10) || 22,
          username: String(c.username ?? ""),
          password: String(c.password ?? ""), // dpapi: 암호문 그대로 — 같은 엔트로피라 복호됨
          keyFile: String(c.keyFile ?? ""),
          encoding: (c.encoding === "utf-8" || c.encoding === "euc-kr" ? c.encoding : "auto") as
            | "auto"
            | "utf-8"
            | "euc-kr",
        }));
      // iDOT 그룹 순서도 반영
      try {
        const idotGroups = JSON.parse(r.groups) as unknown;
        if (Array.isArray(idotGroups))
          for (const g of idotGroups)
            if (typeof g === "string" && !groups.includes(g)) groups.push(g);
      } catch {
        /* ignore */
      }
      const all = JSON.parse(r.servers) as unknown[];
      await finishImport(addImported(conns), all.length - conns.length);
    } catch (e) {
      await confirmBox(t("sites.import"), String(e));
    }
  });

  // 파일 가져오기 (iDrop/iDOT CSV, Xshell .xsh/.xts)
  const fileInput = document.getElementById("sf-import-file") as HTMLInputElement;
  document.getElementById("sf-import")!.addEventListener("click", () => fileInput.click());
  fileInput.addEventListener("change", async () => {
    if (!fileInput.files?.length) return;
    const res: ImportResult = await importFiles([...fileInput.files]);
    const ssh = res.conns.filter((c: ImportedConn) => c.protocol === "ssh" && c.host);
    await finishImport(addImported(ssh), res.skipped + (res.conns.length - ssh.length));
    fileInput.value = "";
  });

  loadForm(null);
}
