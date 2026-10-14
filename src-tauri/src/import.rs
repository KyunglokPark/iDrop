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
