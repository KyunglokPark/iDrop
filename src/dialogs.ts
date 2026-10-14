// 접속 과정의 대화상자들 — 호스트 키 확인, 인증 입력(auth-prompt).
// 백엔드가 이벤트를 보내고 oneshot으로 답을 기다리는 구조라(hostkey.rs/connect.rs),
// 여기서는 이벤트당 모달 하나를 띄우고 답을 invoke로 돌려준다.
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { t } from "./i18n";

interface HostkeyEvt {
  id: string;
  host: string;
  port: number;
  keyType: string;
  fingerprint: string;
  changed: boolean;
}

interface AuthPromptEvt {
  reqId: string;
  kind: "user" | "password" | "passphrase";
  detail: string;
  echo: boolean;
}

function overlay(): HTMLDivElement {
  const ov = document.createElement("div");
  ov.className = "modal-overlay";
  document.body.appendChild(ov);
  return ov;
}

function box(ov: HTMLDivElement, title: string): HTMLDivElement {
  const b = document.createElement("div");
  b.className = "modal-box";
  const h = document.createElement("h3");
  h.textContent = title;
  b.appendChild(h);
  ov.appendChild(b);
  return b;
}

function btn(label: string, cls = ""): HTMLButtonElement {
  const b = document.createElement("button");
  b.type = "button";
  b.textContent = label;
  if (cls) b.className = cls;
  return b;
}

export async function initDialogs() {
  await listen<HostkeyEvt>("ssh-hostkey", (e) => {
    const p = e.payload;
    const ov = overlay();
    const b = box(
      ov,
      p.changed
        ? t("hk.changedTitle")
        : t("hk.title")
    );
    const body = document.createElement("div");
    body.className = "modal-body";
    const warn = p.changed ? `<p class="warn">${t("hk.changedWarn")}</p>` : "";
    body.innerHTML =
      warn +
      `<p>${p.host}:${p.port}</p>` +
      `<p class="mono">${p.keyType}</p><p class="mono">${p.fingerprint}</p>`;
    b.appendChild(body);
    const row = document.createElement("div");
    row.className = "modal-btns";
    const save = btn(t("hk.save"), "primary");
    const once = btn(t("hk.once"));
    const no = btn(t("dlg.cancel"));
    row.append(save, once, no);
    b.appendChild(row);
    const answer = (a: string) => {
      ov.remove();
      void invoke("hostkey_answer", { id: p.id, answer: a });
    };
    save.onclick = () => answer("save");
    once.onclick = () => answer("once");
    no.onclick = () => answer("reject");
  });

  await listen<AuthPromptEvt>("auth-prompt", (e) => {
    const p = e.payload;
    const titles = {
      user: t("auth.user"),
      password: t("auth.password"),
      passphrase: t("auth.passphrase"),
    };
    const ov = overlay();
    const b = box(ov, titles[p.kind] ?? p.kind);
    const body = document.createElement("div");
    body.className = "modal-body";
    const label = document.createElement("p");
    label.textContent = p.detail;
    const input = document.createElement("input");
    input.type = p.echo ? "text" : "password";
    input.spellcheck = false;
    body.append(label, input);
    b.appendChild(body);
    const row = document.createElement("div");
    row.className = "modal-btns";
    const ok = btn(t("dlg.ok"), "primary");
    const no = btn(t("dlg.cancel"));
    row.append(ok, no);
    b.appendChild(row);
    const done = (value: string | null) => {
      ov.remove();
      void invoke("auth_answer", { reqId: p.reqId, value });
    };
    ok.onclick = () => done(input.value);
    no.onclick = () => done(null);
    input.addEventListener("keydown", (ev) => {
      if (ev.key === "Enter") done(input.value);
      if (ev.key === "Escape") done(null);
    });
    input.focus();
  });
}
