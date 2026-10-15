// 서버 호스트 키 확인 (known_hosts).
//
// 처음 보는 서버이거나 저장된 키와 다르면 프런트에 확인 창을 띄우고(ssh-hostkey 이벤트)
// 사용자의 답(hostkey_answer)을 기다린다. 신뢰한 키는 iDOT 전용 known_hosts
// (앱 데이터 폴더)에 OpenSSH 형식으로 저장한다. 사용자의 ~/.ssh/known_hosts도
// 읽기 전용으로 함께 참고한다 (이미 OpenSSH로 접속해 본 서버는 묻지 않는다).

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, OnceLock};

use russh::keys::ssh_key::{HashAlg, PublicKey};
use russh::keys::PublicKeyOrCertificate;
use tauri::{AppHandle, Emitter, Manager};
use tokio::sync::oneshot;

/// 사용자의 답: 거부 / 이번만 허용 / 허용하고 저장
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Answer {
    Reject,
    Once,
    Save,
}

fn waiters() -> &'static Mutex<HashMap<String, oneshot::Sender<Answer>>> {
    static W: OnceLock<Mutex<HashMap<String, oneshot::Sender<Answer>>>> = OnceLock::new();
    W.get_or_init(|| Mutex::new(HashMap::new()))
}

/// 프런트의 확인 창 답. 기다리는 세션이 없으면 무시한다.
pub fn answer(id: &str, a: Answer) {
    if let Some(tx) = waiters().lock().unwrap().remove(id) {
        let _ = tx.send(a);
    }
}

/// 탭이 닫히면 대기 중인 확인을 거부로 끝낸다.
#[allow(dead_code)]
pub fn cancel(id: &str) {
    waiters().lock().unwrap().remove(id);
}

#[derive(Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct Prompt {
    id: String,
    host: String,
    port: u16,
    key_type: String,
    fingerprint: String,
    /// 저장된 키와 다름 (중간자 공격 가능성 경고)
    changed: bool,
}

/// iDOT 전용 known_hosts 경로
pub fn own_path(app: &AppHandle) -> Option<PathBuf> {
    app.path().app_data_dir().ok().map(|d| d.join("known_hosts"))
}

/// 사용자의 OpenSSH known_hosts (읽기 전용)
fn user_path() -> Option<PathBuf> {
    let home = std::env::var_os("USERPROFILE").or_else(|| std::env::var_os("HOME"))?;
    Some(PathBuf::from(home).join(".ssh").join("known_hosts"))
}

#[derive(PartialEq, Debug)]
pub enum Known {
    Match,
    Unknown,
    Changed,
}

/// 한 파일에서 host:port의 키 상태를 본다. 파일이 없거나 읽을 수 없으면 Unknown.
pub fn lookup(path: &std::path::Path, host: &str, port: u16, key: &PublicKey) -> Known {
    match russh::keys::check_known_hosts_path(host, port, key, path) {
        Ok(true) => Known::Match,
        Ok(false) => Known::Unknown,
        Err(russh::keys::Error::KeyChanged { .. }) => Known::Changed,
        Err(_) => Known::Unknown,
    }
}

fn host_field(host: &str, port: u16) -> String {
    if port == 22 {
        host.to_string()
    } else {
        format!("[{host}]:{port}")
    }
}

/// host:port의 기존 줄(평문 호스트 이름만)을 지우고 새 키를 추가한다.
pub fn save(path: &std::path::Path, host: &str, port: u16, key: &PublicKey) -> std::io::Result<()> {
    let field = host_field(host, port);
    let old = std::fs::read_to_string(path).unwrap_or_default();
    let mut out = String::new();
    for line in old.lines() {
        let hosts = line.split_whitespace().next().unwrap_or("");
        if !line.starts_with('#') && hosts.split(',').any(|h| h == field) {
            continue; // 바뀐 키의 옛 줄
        }
        out.push_str(line);
        out.push('\n');
    }
    let openssh = key
        .to_openssh()
        .map_err(|e| std::io::Error::other(e.to_string()))?;
    out.push_str(&format!("{field} {openssh}\n"));
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::write(path, out)
}

/// 서버가 보여준 키를 확인한다. true면 접속을 계속한다.
/// `prompting`은 사용자 답을 기다리는 동안 핸드셰이크 시간 제한을 멈추는 데 쓴다.
pub async fn verify(
    app: &AppHandle,
    id: &str,
    host: &str,
    port: u16,
    key: &PublicKeyOrCertificate,
    prompting: &AtomicBool,
) -> bool {
    let key: PublicKey = match key {
        PublicKeyOrCertificate::PublicKey { key, .. } => key.clone(),
        // 호스트 인증서는 CA 검증 대신 인증서 안의 키를 호스트 키로 다룬다
        PublicKeyOrCertificate::Certificate(c) => PublicKey::from(c.public_key().clone()),
    };
    let own = own_path(app);
    let mine = own.as_deref().map(|p| lookup(p, host, port, &key)).unwrap_or(Known::Unknown);
    if mine == Known::Match {
        return true;
    }
    let theirs = user_path().map(|p| lookup(&p, host, port, &key)).unwrap_or(Known::Unknown);
    if theirs == Known::Match && mine == Known::Unknown {
        return true;
    }
    let changed = mine == Known::Changed || theirs == Known::Changed;

    let (tx, rx) = oneshot::channel();
    waiters().lock().unwrap().insert(id.to_string(), tx);
    prompting.store(true, Ordering::SeqCst);
    let _ = app.emit(
        "ssh-hostkey",
        Prompt {
            id: id.to_string(),
            host: host.to_string(),
            port,
            key_type: key.algorithm().to_string(),
            fingerprint: key.fingerprint(HashAlg::Sha256).to_string(),
            changed,
        },
    );
    let ans = rx.await.unwrap_or(Answer::Reject);
    prompting.store(false, Ordering::SeqCst);
    match ans {
        Answer::Reject => false,
        Answer::Once => true,
        Answer::Save => {
            if let Some(p) = own {
                let _ = save(&p, host, port, &key);
            }
            true
        }
    }
}
