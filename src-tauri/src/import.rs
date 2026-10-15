// 다른 터미널 프로그램의 세션 가져오기
//  - PuTTY: 레지스트리 HKCU\Software\SimonTatham\PuTTY\Sessions (reg.exe로 읽는다)
//  - Xshell: 문서\NetSarang Computer\<버전>\Xshell\Sessions 폴더의 .xsh 파일들
// 해석(필드 매핑)은 프런트(sessionio.ts)가 하고, 여기서는 원본 값만 모아 넘긴다.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// reg.exe 출력 (콘솔 코드페이지 — 한글 Windows는 CP949). 실패하면 빈 문자열.
pub fn reg_query(key: &str, recursive: bool) -> String {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        let mut cmd = std::process::Command::new("reg.exe");
        cmd.arg("query").arg(key);
        if recursive {
            cmd.arg("/s");
        }
        match cmd.creation_flags(CREATE_NO_WINDOW).output() {
            Ok(o) => decode_console(&o.stdout),
            Err(_) => String::new(),
        }
    }
    #[cfg(not(windows))]
    {
        let _ = (key, recursive);
        String::new()
    }
}

fn decode_console(b: &[u8]) -> String {
    match std::str::from_utf8(b) {
        Ok(s) => s.to_string(),
        Err(_) => encoding_rs::EUC_KR.decode(b).0.into_owned(),
    }
}

/// PuTTY 세션 이름의 %XX 이스케이프 풀기
fn unescape(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' && i + 2 < b.len() {
            if let Some(v) = std::str::from_utf8(&b[i + 1..i + 3]).ok().and_then(|h| u8::from_str_radix(h, 16).ok()) {
                out.push(v);
                i += 3;
                continue;
            }
        }
        out.push(b[i]);
        i += 1;
    }
    match String::from_utf8(out.clone()) {
        Ok(s) => s,
        Err(_) => encoding_rs::EUC_KR.decode(&out).0.into_owned(),
    }
}

/// `reg query ... /s` 출력 → [(세션 이름, {값 이름: 값})]
pub fn parse_putty(out: &str) -> Vec<BTreeMap<String, String>> {
    const ROOT: &str = "\\Software\\SimonTatham\\PuTTY\\Sessions\\";
    let mut list: Vec<BTreeMap<String, String>> = Vec::new();
    for line in out.lines() {
        let line = line.trim_end();
        if line.is_empty() {
            continue;
        }
        if !line.starts_with(' ') {
            // 키 줄: HKEY_CURRENT_USER\Software\SimonTatham\PuTTY\Sessions\이름
            if let Some(i) = line.find(ROOT) {
                let name = &line[i + ROOT.len()..];
                if !name.is_empty() && !name.contains('\\') {
                    let mut m = BTreeMap::new();
                    m.insert("_name".to_string(), unescape(name));
                    list.push(m);
                    continue;
                }
            }
            continue;
        }
        // 값 줄: "    HostName    REG_SZ    10.0.0.1"
        let Some(cur) = list.last_mut() else { continue };
        let t = line.trim_start();
        let mut parts = t.splitn(2, "    ");
        let (Some(name), Some(rest)) = (parts.next(), parts.next()) else { continue };
        let rest = rest.trim_start();
        let (typ, val) = match rest.split_once("    ") {
            Some((ty, v)) => (ty.trim(), v.to_string()),
            None => (rest.trim(), String::new()),
        };
        let val = if typ == "REG_DWORD" {
            u64::from_str_radix(val.trim().trim_start_matches("0x"), 16)
                .map(|n| n.to_string())
                .unwrap_or_default()
        } else {
            val
        };
        cur.insert(name.trim().to_string(), val);
    }
    list.retain(|m| m.len() > 1);
    list
}

pub fn putty_sessions() -> Vec<BTreeMap<String, String>> {
    parse_putty(&reg_query(r"HKCU\Software\SimonTatham\PuTTY\Sessions", true))
}

#[derive(serde::Serialize)]
pub struct XshFile {
    /// Sessions 폴더 기준 상대 경로 ("그룹/이름.xsh")
    pub path: String,
    pub data: Vec<u8>,
}

fn collect_xsh(root: &Path, dir: &Path, out: &mut Vec<XshFile>, depth: usize) {
    if depth > 8 {
        return;
    }
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    for e in rd.flatten() {
        let p = e.path();
        if p.is_dir() {
            collect_xsh(root, &p, out, depth + 1);
        } else if p
            .extension()
            .map(|x| x.eq_ignore_ascii_case("xsh"))
            .unwrap_or(false)
        {
            if let (Ok(data), Ok(rel)) = (std::fs::read(&p), p.strip_prefix(root)) {
                let path = rel.to_string_lossy().replace('\\', "/");
                out.push(XshFile { path, data });
            }
        }
    }
}

/// Xshell 세션 폴더 후보 (문서 폴더 / OneDrive 문서 폴더의 NetSarang Computer\<버전>)
fn xshell_roots() -> Vec<PathBuf> {
    let mut docs: Vec<PathBuf> = Vec::new();
    // 문서 폴더 위치가 바뀌었을 수 있다 (User Shell Folders\Personal)
    let reg = reg_query(r"HKCU\Software\Microsoft\Windows\CurrentVersion\Explorer\Shell Folders", false);
    for line in reg.lines() {
        let t = line.trim();
        if let Some(rest) = t.strip_prefix("Personal") {
            if let Some((_, v)) = rest.trim_start().split_once("    ") {
                docs.push(PathBuf::from(v.trim()));
            }
        }
    }
    if let Some(h) = std::env::var_os("USERPROFILE") {
        docs.push(PathBuf::from(&h).join("Documents"));
    }
    if let Some(od) = std::env::var_os("OneDrive") {
        docs.push(PathBuf::from(od).join("Documents"));
    }
    let mut roots = Vec::new();
    for d in docs {
        let base = d.join("NetSarang Computer");
        let Ok(rd) = std::fs::read_dir(&base) else { continue };
        // 버전 폴더(6, 7, 8 …) — 높은 버전 우선
        let mut vers: Vec<(u32, PathBuf)> = rd
            .flatten()
            .filter_map(|e| {
                let v = e.file_name().to_string_lossy().parse::<u32>().ok()?;
                let s = e.path().join("Xshell").join("Sessions");
                s.is_dir().then_some((v, s))
            })
            .collect();
        vers.sort_by(|a, b| b.0.cmp(&a.0));
        for (_, s) in vers {
            if !roots.contains(&s) {
                roots.push(s);
            }
        }
    }
    roots
}

/// 가장 높은 버전의 Xshell 세션 폴더에서 .xsh를 모두 읽는다.
pub fn xshell_sessions() -> (Option<String>, Vec<XshFile>) {
    for root in xshell_roots() {
        let mut out = Vec::new();
        collect_xsh(&root, &root, &mut out, 0);
        if !out.is_empty() {
            return (Some(root.to_string_lossy().into_owned()), out);
        }
    }
    (None, Vec::new())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn putty_parse() {
        let out = "\r\nHKEY_CURRENT_USER\\Software\\SimonTatham\\PuTTY\\Sessions\\Default%20Settings\r\n    HostName    REG_SZ    \r\n\r\nHKEY_CURRENT_USER\\Software\\SimonTatham\\PuTTY\\Sessions\\%EC%9A%B4%EC%98%81%201\r\n    HostName    REG_SZ    10.0.0.5\r\n    PortNumber    REG_DWORD    0x16\r\n    Protocol    REG_SZ    ssh\r\n    UserName    REG_SZ    root\r\n    PublicKeyFile    REG_SZ    C:\\keys\\a b.ppk\r\n";
        let l = parse_putty(out);
        assert_eq!(l.len(), 2);
        assert_eq!(l[1]["_name"], "운영 1");
        assert_eq!(l[1]["HostName"], "10.0.0.5");
        assert_eq!(l[1]["PortNumber"], "22");
        assert_eq!(l[1]["PublicKeyFile"], "C:\\keys\\a b.ppk");
        assert_eq!(l[0]["_name"], "Default Settings");
    }
}

// ---- iDOT localStorage 직접 읽기 ---------------------------------------------
// iDOT(자매 앱)의 WebView2 localStorage는 LevelDB 파일에 있다. 정식 LevelDB
// 리더 없이, 파일 바이트에서 키("idot.servers" 등) 출현 지점을 찾아 바로 뒤의
// 값(JSON)을 괄호 균형으로 잘라낸다. Chromium localStorage 값은 첫 바이트가
// 0x01이면 Latin-1(ASCII JSON은 그대로), 0x00이면 UTF-16LE다.
// 같은 파일에 과거 버전 레코드가 여럿 남아 있으므로 "가장 마지막 출현"을 쓴다
// (파일은 수정 시각 순으로 스캔 — .log가 최신 쓰기를 담는다).
// 비밀번호(dpapi: 암호문)는 iDrop과 DPAPI 엔트로피가 같아 그대로 복호된다.

fn idot_leveldb_dir() -> Option<std::path::PathBuf> {
    let base = std::env::var("LOCALAPPDATA").ok()?;
    let p = std::path::Path::new(&base)
        .join("com.nezzard.idot")
        .join("EBWebView")
        .join("Default")
        .join("Local Storage")
        .join("leveldb");
    p.is_dir().then_some(p)
}

/// JSON 배열/객체를 괄호 균형으로 잘라낸다 (문자열·이스케이프 고려). step=1(u8) 또는 2(UTF-16LE).
fn balanced_json(bytes: &[u8], start: usize, step: usize) -> Option<String> {
    let open = *bytes.get(start)?;
    let close = match open {
        b'[' => b']',
        b'{' => b'}',
        _ => return None,
    };
    let mut depth = 0i32;
    let mut in_str = false;
    let mut esc = false;
    let mut out = Vec::new();
    let mut i = start;
    while i < bytes.len() {
        if step == 2 && bytes.get(i + 1).copied().unwrap_or(1) != 0 {
            return None; // UTF-16LE 상위 바이트가 0이 아니면 ASCII JSON이 아님 — 한글 등
        }
        let b = bytes[i];
        out.push(b);
        if in_str {
            if esc {
                esc = false;
            } else if b == b'\\' {
                esc = true;
            } else if b == b'"' {
                in_str = false;
            }
        } else if b == b'"' {
            in_str = true;
        } else if b == open {
            depth += 1;
        } else if b == close {
            depth -= 1;
            if depth == 0 {
                return String::from_utf8(out).ok();
            }
        }
        i += step;
    }
    None
}

/// UTF-16LE 구간용 — 한글이 섞인 JSON도 제대로 디코드한다.
fn balanced_json_u16(bytes: &[u8], start: usize) -> Option<String> {
    let units: Vec<u16> = bytes[start..]
        .chunks_exact(2)
        .map(|c| u16::from_le_bytes([c[0], c[1]]))
        .collect();
    let open = *units.first()? as u8 as char;
    let close = match open {
        '[' => ']',
        '{' => '}',
        _ => return None,
    };
    let mut depth = 0i32;
    let mut in_str = false;
    let mut esc = false;
    for (n, &u) in units.iter().enumerate() {
        let c = char::from_u32(u as u32).unwrap_or('\u{fffd}');
        if in_str {
            if esc {
                esc = false;
            } else if c == '\\' {
                esc = true;
            } else if c == '"' {
                in_str = false;
            }
        } else if c == '"' {
            in_str = true;
        } else if c == open {
            depth += 1;
        } else if c == close {
            depth -= 1;
            if depth == 0 {
                return String::from_utf16(&units[..=n]).ok();
            }
        }
    }
    None
}

/// 파일 바이트에서 `key` 레코드의 JSON 값 — 마지막 출현을 돌려준다.
fn extract_value(bytes: &[u8], key: &[u8]) -> Option<String> {
    let mut found = None;
    let mut from = 0;
    while let Some(rel) = bytes[from..].windows(key.len()).position(|w| w == key) {
        let after = from + rel + key.len();
        // 키 직후 몇 바이트 안에 값 마커(0x00/0x01)와 '['가 온다
        for j in after..(after + 6).min(bytes.len().saturating_sub(1)) {
            match bytes[j] {
                0x01 => {
                    if bytes.get(j + 1) == Some(&b'[') {
                        if let Some(s) = balanced_json(bytes, j + 1, 1) {
                            if serde_json::from_str::<serde_json::Value>(&s).is_ok() {
                                found = Some(s);
                            }
                        }
                    }
                }
                0x00 => {
                    if bytes.get(j + 1) == Some(&b'[') {
                        if let Some(s) = balanced_json_u16(bytes, j + 1) {
                            if serde_json::from_str::<serde_json::Value>(&s).is_ok() {
                                found = Some(s);
                            }
                        }
                    }
                }
                _ => {}
            }
        }
        from = after;
    }
    found
}

/// iDOT 저장 세션(servers)과 그룹 순서(groups)의 JSON 문자열.
pub fn idot_sessions() -> Result<(String, String), String> {
    let dir = idot_leveldb_dir().ok_or("iDOT 저장 데이터를 찾을 수 없습니다 (iDOT가 설치·실행된 적이 있어야 합니다)")?;
    let mut files: Vec<(std::time::SystemTime, std::path::PathBuf)> = std::fs::read_dir(&dir)
        .map_err(|e| e.to_string())?
        .flatten()
        .filter(|e| {
            let n = e.file_name().to_string_lossy().into_owned();
            n.ends_with(".log") || n.ends_with(".ldb")
        })
        .filter_map(|e| Some((e.metadata().ok()?.modified().ok()?, e.path())))
        .collect();
    files.sort_by_key(|(t, _)| *t); // 오래된 파일 → 최신 파일 순 (나중 발견이 이긴다)

    let mut servers = None;
    let mut groups = None;
    for (_, p) in files {
        if let Ok(bytes) = std::fs::read(&p) {
            if let Some(s) = extract_value(&bytes, b"idot.servers") {
                servers = Some(s);
            }
            if let Some(g) = extract_value(&bytes, b"idot.groups") {
                groups = Some(g);
            }
        }
    }
    let servers = servers.ok_or("iDOT 세션 목록을 찾지 못했습니다")?;
    Ok((servers, groups.unwrap_or_else(|| "[]".into())))
}
