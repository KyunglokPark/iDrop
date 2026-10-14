// 세션 목록 내보내기 / 가져오기.
//  - 내보내기: CSV (UTF-8 BOM — 엑셀에서 한글이 깨지지 않게)
//  - 가져오기: iDOT CSV, Xshell 세션(.xsh 파일, "세션 내보내기"로 만든 .xts 압축 파일,
//    또는 Xshell 세션 폴더), PuTTY 저장 세션(레지스트리 값 — 백엔드가 읽어 넘긴다)
// 외부 라이브러리 없이 처리한다 (.xts는 ZIP — 브라우저 내장 DecompressionStream으로 풀기).

export interface SerialCfg {
  port: string; // "COM3"
  baud: number;
  dataBits: number; // 5–8
  parity: "none" | "odd" | "even";
  stopBits: number; // 1 | 2
  flow: "none" | "xonxoff" | "rtscts";
}

export const DEFAULT_SERIAL: SerialCfg = { port: "COM1", baud: 9600, dataBits: 8, parity: "none", stopBits: 1, flow: "none" };

export interface ImportedConn {
  name: string;
  group: string;
  protocol: "ssh" | "telnet" | "serial";
  host: string;
  port: number;
  username: string;
  password: string;
  encoding: "auto" | "utf-8" | "euc-kr";
  ansi: boolean;
  keyFile?: string; // 개인키 파일 경로
  serial?: SerialCfg;
}

export interface ImportResult {
  conns: ImportedConn[];
  skipped: number; // 지원하지 않는 프로토콜(SFTP·시리얼 등)·호스트 없음 등
}

// ---- 공통: 바이트 → 문자열 ----------------------------------------------------

/** BOM으로 UTF-16/UTF-8을 가리고, BOM이 없으면 UTF-8 → 실패 시 EUC-KR(CP949). */
export function decodeText(b: Uint8Array): string {
  if (b.length >= 2 && b[0] === 0xff && b[1] === 0xfe) return new TextDecoder("utf-16le").decode(b.subarray(2));
  if (b.length >= 2 && b[0] === 0xfe && b[1] === 0xff) return new TextDecoder("utf-16be").decode(b.subarray(2));
  if (b.length >= 3 && b[0] === 0xef && b[1] === 0xbb && b[2] === 0xbf) return new TextDecoder("utf-8").decode(b.subarray(3));
  try {
    return new TextDecoder("utf-8", { fatal: true }).decode(b);
  } catch {
    return new TextDecoder("euc-kr").decode(b); // 한글 Windows 엑셀이 저장한 CSV 등
  }
}

function normEncoding(v: string): ImportedConn["encoding"] {
  const s = v.trim().toLowerCase();
  if (["utf-8", "utf8", "65001"].includes(s)) return "utf-8";
  if (["euc-kr", "euckr", "cp949", "949", "51949", "ks_c_5601-1987", "ks_c_5601"].includes(s)) return "euc-kr";
  return "auto";
}

function normProtocol(v: string): ImportedConn["protocol"] | null {
  const s = v.trim().toLowerCase();
  if (s === "" || s === "ssh" || s === "ssh2") return "ssh";
  if (s === "telnet") return "telnet";
  if (s === "serial") return "serial";
  return null; // SFTP, RLOGIN, RAW, LOCAL 등은 iDOT에서 지원하지 않음
}

type Fields = Partial<Record<Exclude<keyof ImportedConn, "serial">, string>>;

function makeConn(f: Fields, serial?: SerialCfg): ImportedConn | null {
  const protocol = normProtocol(f.protocol ?? "");
  const host = (f.host ?? "").trim();
  if (!protocol) return null;
  if (protocol === "serial") {
    if (!serial || !serial.port) return null;
    return {
      name: (f.name ?? "").trim() || serial.port,
      group: (f.group ?? "").trim(),
      protocol,
      host: "",
      port: 0,
      username: "",
      password: "",
      encoding: normEncoding(f.encoding ?? ""),
      ansi: true,
      serial,
    };
  }
  if (!host) return null;
  const port = parseInt(f.port ?? "", 10);
  const ansi = (f.ansi ?? "").trim().toLowerCase();
  return {
    name: (f.name ?? "").trim() || host,
    group: (f.group ?? "").trim(),
    protocol,
    host,
    port: Number.isFinite(port) && port > 0 && port < 65536 ? port : protocol === "telnet" ? 23 : 22,
    username: (f.username ?? "").trim(),
    password: f.password ?? "",
    encoding: normEncoding(f.encoding ?? ""),
    ansi: !["false", "0", "no", "n", "off"].includes(ansi),
    keyFile: (f.keyFile ?? "").trim(),
  };
}

// ---- CSV ----------------------------------------------------------------------

const CSV_COLUMNS: Exclude<keyof ImportedConn, "serial">[] = [
  "name", "group", "protocol", "host", "port", "username", "password", "encoding", "ansi", "keyFile",
];

// 가져오기 시 머리글 별칭 (한글 머리글로 직접 만든 CSV도 받는다)
const HEADER_ALIASES: Record<string, Exclude<keyof ImportedConn, "serial">> = {
  name: "name", 별칭: "name", 이름: "name",
  group: "group", 그룹: "group",
  protocol: "protocol", 프로토콜: "protocol",
  host: "host", "호스트": "host", "호스트 (ip)": "host", ip: "host",
  port: "port", 포트: "port",
  username: "username", user: "username", id: "username", "접속 id": "username", 아이디: "username",
  password: "password", 비밀번호: "password",
  encoding: "encoding", 인코딩: "encoding",
  ansi: "ansi",
  keyfile: "keyFile", key: "keyFile", 개인키: "keyFile",
};

function csvField(v: string): string {
  return /[",\r\n]/.test(v) || /^\s|\s$/.test(v) ? `"${v.replace(/"/g, '""')}"` : v;
}

export function toCsv(list: ImportedConn[], includePassword: boolean): string {
  const rows = [CSV_COLUMNS.join(",")];
  for (const c of list) {
    if (c.protocol === "serial") continue; // 시리얼 설정은 CSV 열에 담지 않는다
    rows.push(
      CSV_COLUMNS.map((k) => {
        if (k === "password" && !includePassword) return "";
        return csvField(String(c[k] ?? ""));
      }).join(","),
    );
  }
  return "﻿" + rows.join("\r\n") + "\r\n";
}

/** RFC 4180 CSV 파서 (따옴표 안의 쉼표·줄바꿈·"" 처리). */
export function parseCsv(text: string): string[][] {
  const rows: string[][] = [];
  let row: string[] = [];
  let field = "";
  let quoted = false;
  for (let i = 0; i < text.length; i++) {
    const ch = text[i];
    if (quoted) {
      if (ch === '"') {
        if (text[i + 1] === '"') {
          field += '"';
          i++;
        } else quoted = false;
      } else field += ch;
    } else if (ch === '"') quoted = true;
    else if (ch === ",") {
      row.push(field);
      field = "";
    } else if (ch === "\n" || ch === "\r") {
      if (ch === "\r" && text[i + 1] === "\n") i++;
      row.push(field);
      rows.push(row);
      row = [];
      field = "";
    } else field += ch;
  }
  if (field !== "" || row.length) {
    row.push(field);
    rows.push(row);
  }
  return rows.filter((r) => r.some((v) => v.trim() !== ""));
}

function csvToConns(text: string): ImportResult {
  const rows = parseCsv(text.replace(/^﻿/, ""));
  if (!rows.length) return { conns: [], skipped: 0 };
  const map = rows[0].map((h) => HEADER_ALIASES[h.trim().toLowerCase()] ?? null);
  const conns: ImportedConn[] = [];
  let skipped = 0;
  for (const r of rows.slice(1)) {
    const f: Fields = {};
    map.forEach((k, i) => {
      if (k) f[k] = r[i] ?? "";
    });
    const c = makeConn(f);
    if (c) conns.push(c);
    else skipped++;
  }
  return { conns, skipped };
}

// ---- Xshell (.xsh / .xts) -----------------------------------------------------

/** INI → { "section": { "key": value } } (섹션·키는 소문자로) */
function parseIni(text: string): Record<string, Record<string, string>> {
  const out: Record<string, Record<string, string>> = {};
  let cur = out[""] = {} as Record<string, string>;
  for (const raw of text.split(/\r?\n/)) {
    const line = raw.trim();
    if (!line || line.startsWith(";") || line.startsWith("#")) continue;
    const sec = /^\[(.+)\]$/.exec(line);
    if (sec) {
      cur = out[sec[1].trim().toLowerCase()] ??= {};
      continue;
    }
    const eq = line.indexOf("=");
    if (eq > 0) cur[line.slice(0, eq).trim().toLowerCase()] = line.slice(eq + 1).trim();
  }
  return out;
}

function xshToConn(name: string, group: string, text: string): ImportedConn | null {
  const ini = parseIni(text);
  const conn = ini["connection"] ?? {};
  const auth = ini["connection:authentication"] ?? {};
  // 인코딩 키 이름은 버전마다 달라 모든 섹션에서 찾는다
  let encoding = "";
  for (const sec of Object.values(ini)) {
    for (const k of ["codepage", "encoding", "charset"]) {
      if (sec[k]) encoding = sec[k];
    }
  }
  // Xshell 비밀번호는 Xshell 전용 방식으로 암호화돼 있어 가져오지 않는다 (접속 시 입력)
  return makeConn({
    name,
    group,
    protocol: conn["protocol"] ?? "SSH",
    host: conn["host"] ?? "",
    port: conn["port"] ?? "",
    username: auth["username"] ?? conn["username"] ?? "",
    encoding,
  });
}

interface ZipEntry {
  path: string;
  data: Uint8Array;
}

async function inflateRaw(data: Uint8Array): Promise<Uint8Array> {
  const ds = new DecompressionStream("deflate-raw");
  const buf = await new Response(new Blob([new Uint8Array(data)]).stream().pipeThrough(ds)).arrayBuffer();
  return new Uint8Array(buf);
}

function zipName(bytes: Uint8Array, utf8Flag: boolean): string {
  if (utf8Flag) return new TextDecoder("utf-8").decode(bytes);
  try {
    return new TextDecoder("utf-8", { fatal: true }).decode(bytes);
  } catch {
    return new TextDecoder("euc-kr").decode(bytes); // 한글 Windows에서 만든 ZIP의 파일 이름
  }
}

/** 최소 ZIP 리더 — 중앙 디렉터리를 읽고 저장(0)/deflate(8) 항목만 푼다. */
async function readZip(b: Uint8Array): Promise<ZipEntry[]> {
  const dv = new DataView(b.buffer, b.byteOffset, b.byteLength);
  let eocd = -1;
  for (let i = b.length - 22; i >= Math.max(0, b.length - 22 - 65535); i--) {
    if (dv.getUint32(i, true) === 0x06054b50) {
      eocd = i;
      break;
    }
  }
  if (eocd < 0) throw new Error("not a zip");
  const count = dv.getUint16(eocd + 10, true);
  let p = dv.getUint32(eocd + 16, true);
  const out: ZipEntry[] = [];
  for (let n = 0; n < count; n++) {
    if (dv.getUint32(p, true) !== 0x02014b50) break;
    const flags = dv.getUint16(p + 8, true);
    const method = dv.getUint16(p + 10, true);
    const csize = dv.getUint32(p + 20, true);
    const nameLen = dv.getUint16(p + 28, true);
    const extraLen = dv.getUint16(p + 30, true);
    const commentLen = dv.getUint16(p + 32, true);
    const local = dv.getUint32(p + 42, true);
    const path = zipName(b.subarray(p + 46, p + 46 + nameLen), (flags & 0x800) !== 0).replace(/\\/g, "/");
    p += 46 + nameLen + extraLen + commentLen;
    if (path.endsWith("/") || (flags & 1)) continue; // 폴더 항목, 암호화된 항목
    const lNameLen = dv.getUint16(local + 26, true);
    const lExtraLen = dv.getUint16(local + 28, true);
    const start = local + 30 + lNameLen + lExtraLen;
    const raw = b.subarray(start, start + csize);
    if (method === 0) out.push({ path, data: raw });
    else if (method === 8) out.push({ path, data: await inflateRaw(raw) });
  }
  return out;
}

function baseName(path: string): string {
  return (path.split("/").pop() ?? path).replace(/\.xsh$/i, "");
}

/** .xts(ZIP) 안의 .xsh들 — 폴더 구조를 그룹으로 (공통 상위 폴더는 떼어낸다).
 *  Xshell 세션 폴더에서 읽은 파일 목록({상대 경로, 내용})도 같은 방식으로 처리한다. */
export function xtsToConns(entries: ZipEntry[]): ImportResult {
  const xsh = entries.filter((e) => /\.xsh$/i.test(e.path));
  const dirs = xsh.map((e) => e.path.split("/").slice(0, -1));
  let common = 0;
  if (dirs.length) {
    const first = dirs[0];
    while (common < first.length && dirs.every((d) => d[common] === first[common])) common++;
  }
  const conns: ImportedConn[] = [];
  let skipped = 0;
  xsh.forEach((e, i) => {
    const group = dirs[i].slice(common).join(" / ");
    const c = xshToConn(baseName(e.path), group, decodeText(e.data));
    if (c) conns.push(c);
    else skipped++;
  });
  return { conns, skipped };
}

// ---- PuTTY (레지스트리) ------------------------------------------------------------

function puttyEncoding(v: string): ImportedConn["encoding"] {
  const s = v.trim().toLowerCase();
  if (s.startsWith("utf-8") || s === "utf8") return "utf-8";
  if (s.includes("949") || s.includes("euc-kr") || s.includes("korean")) return "euc-kr";
  return "auto";
}

/** PuTTY 세션 값 목록(_name + 레지스트리 값) → 세션. "Default Settings"는 건너뛴다. */
export function puttyToConns(list: Record<string, string>[]): ImportResult {
  const conns: ImportedConn[] = [];
  let skipped = 0;
  for (const v of list) {
    const name = v["_name"] ?? "";
    if (name === "Default Settings") continue;
    const proto = (v["Protocol"] ?? "ssh").toLowerCase();
    let c: ImportedConn | null;
    if (proto === "serial") {
      const halves = parseInt(v["SerialStopHalfbits"] ?? "2", 10);
      const parity = ["none", "odd", "even"][parseInt(v["SerialParity"] ?? "0", 10)] as SerialCfg["parity"] | undefined;
      const flow = ["none", "xonxoff", "rtscts"][parseInt(v["SerialFlowControl"] ?? "1", 10)] as SerialCfg["flow"] | undefined;
      c = makeConn(
        { name, group: "PuTTY", protocol: "serial", encoding: puttyEncoding(v["LineCodePage"] ?? "") },
        {
          port: (v["SerialLine"] ?? "").trim(),
          baud: parseInt(v["SerialSpeed"] ?? "9600", 10) || 9600,
          dataBits: parseInt(v["SerialDataBits"] ?? "8", 10) || 8,
          parity: parity ?? "none",
          stopBits: halves >= 3 ? 2 : 1,
          flow: flow ?? "none",
        },
      );
    } else {
      // HostName에 "user@host"를 넣는 경우도 있다
      let host = (v["HostName"] ?? "").trim();
      let user = (v["UserName"] ?? "").trim();
      const at = host.lastIndexOf("@");
      if (at > 0) {
        user ||= host.slice(0, at);
        host = host.slice(at + 1);
      }
      c = makeConn({
        name,
        group: "PuTTY",
        protocol: proto,
        host,
        port: v["PortNumber"] ?? "",
        username: user,
        encoding: puttyEncoding(v["LineCodePage"] ?? ""),
        keyFile: v["PublicKeyFile"] ?? "",
      });
    }
    if (c) conns.push(c);
    else skipped++;
  }
  return { conns, skipped };
}

// ---- 진입점 ---------------------------------------------------------------------

/** 사용자가 고른 파일들(.csv / .xsh / .xts / .zip)을 세션 목록으로 바꾼다. */
export async function importFiles(files: File[]): Promise<ImportResult> {
  const all: ImportResult = { conns: [], skipped: 0 };
  for (const f of files) {
    const bytes = new Uint8Array(await f.arrayBuffer());
    let r: ImportResult;
    if (/\.(xts|zip)$/i.test(f.name)) r = xtsToConns(await readZip(bytes));
    else if (/\.xsh$/i.test(f.name)) {
      const c = xshToConn(baseName(f.name), "", decodeText(bytes));
      r = { conns: c ? [c] : [], skipped: c ? 0 : 1 };
    } else r = csvToConns(decodeText(bytes));
    all.conns.push(...r.conns);
    all.skipped += r.skipped;
  }
  return all;
}
