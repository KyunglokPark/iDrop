// SSH 접속·인증 (iDOT lib.rs의 SSH 계층에서 추출, 헤드리스화).
//
// iDOT는 사용자 입력(사용자명·비밀번호·키 암호문구)을 터미널 프롬프트로 받았지만,
// iDrop에는 터미널이 없으므로 "auth-prompt" 이벤트 → 프런트 대화상자 →
// auth_answer 명령 → oneshot 회신으로 받는다 (hostkey.rs의 waiters 패턴과 동일).
// 인증 순서는 iDOT와 같다: 개인키 → 에이전트 → 비밀번호/keyboard-interactive.
// (허용 안 된 공개키 시도만으로 접속을 끊는 사내 게이트웨이가 있어, 키 인증은
// 개인키·에이전트를 지정한 경우에만 시도한다.)

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use russh::client::{self, AuthResult, KeyboardInteractiveAuthResponse};
use russh::keys::{PrivateKeyWithHashAlg, PublicKeyOrCertificate};
use tauri::{AppHandle, Emitter};
use tokio::sync::oneshot;

use crate::hostkey;
use crate::tr_s;

/// SSH 연결 핸들러 — 서버 호스트 키 확인(known_hosts).
#[derive(Clone)]
pub struct Client {
    pub app: AppHandle,
    pub id: String,
    pub host: String,
    pub port: u16,
    /// 확인 창(호스트 키)이 떠 있는 동안 핸드셰이크 시간 제한을 멈춘다
    pub prompting: Arc<AtomicBool>,
}

impl client::Handler for Client {
    type Error = russh::Error;

    async fn check_server_key(
        &mut self,
        server_public_key: &PublicKeyOrCertificate,
    ) -> Result<bool, Self::Error> {
        Ok(hostkey::verify(
            &self.app,
            &self.id,
            &self.host,
            self.port,
            server_public_key,
            &self.prompting,
        )
        .await)
    }
}

pub type SshHandle = client::Handle<Client>;

/// 접속 정보 (사이트 관리자/퀵커넥트에서 온다)
#[derive(Clone, Default, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Hop {
    pub host: String,
    pub port: u16,
    #[serde(default)]
    pub username: String,
    #[serde(default)]
    pub password: String,
    /// 개인키 파일 경로 (OpenSSH / PEM / PuTTY .ppk)
    #[serde(default)]
    pub key_file: String,
    /// Pageant / OpenSSH 에이전트의 키로 인증
    #[serde(default)]
    pub use_agent: bool,
}

// ---- 사용자 입력 프롬프트 (이벤트 + oneshot) ---------------------------------

#[derive(serde::Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct AuthPrompt {
    req_id: String,
    /// "user" | "password" | "passphrase"
    kind: String,
    /// 어떤 접속/키에 대한 프롬프트인지 (대화상자 제목용)
    detail: String,
    /// 입력 에코 여부 (사용자명 true, 비밀번호류 false)
    echo: bool,
}

static NEXT_REQ: AtomicU64 = AtomicU64::new(1);

fn waiters() -> &'static Mutex<HashMap<String, oneshot::Sender<Option<String>>>> {
    static W: std::sync::OnceLock<Mutex<HashMap<String, oneshot::Sender<Option<String>>>>> =
        std::sync::OnceLock::new();
    W.get_or_init(|| Mutex::new(HashMap::new()))
}

/// 프런트 대화상자로 한 값을 입력받는다. None = 사용자가 취소.
pub async fn prompt(app: &AppHandle, kind: &str, detail: &str, echo: bool) -> Option<String> {
    let req_id = format!("auth-{}", NEXT_REQ.fetch_add(1, Ordering::Relaxed));
    let (tx, rx) = oneshot::channel();
    waiters().lock().unwrap().insert(req_id.clone(), tx);
    let _ = app.emit(
        "auth-prompt",
        AuthPrompt {
            req_id: req_id.clone(),
            kind: kind.into(),
            detail: detail.into(),
            echo,
        },
    );
    let ans = rx.await.unwrap_or(None);
    waiters().lock().unwrap().remove(&req_id);
    ans
}

/// 프런트의 대화상자 답 (value 없으면 취소)
#[tauri::command]
pub fn auth_answer(req_id: String, value: Option<String>) {
    if let Some(tx) = waiters().lock().unwrap().remove(&req_id) {
        let _ = tx.send(value);
    }
}

fn cancelled() -> String {
    tr_s!("접속이 취소되었습니다", "connection cancelled")
}

// ---- TCP + 핸드셰이크 (iDOT에서 그대로) --------------------------------------

/// 호스트의 모든 주소를 IPv4 우선으로 순차 시도해 TCP 연결을 만든다.
/// (Windows에서 localhost가 ::1로 먼저 풀려 블랙홀이 되는 환경 대비 — iDOT와 동일)
pub async fn tcp_connect(host: &str, port: u16) -> Result<tokio::net::TcpStream, String> {
    let mut addrs: Vec<SocketAddr> = tokio::net::lookup_host((host, port))
        .await
        .map_err(|e| tr_s!(format!("{host}: 주소 확인 실패 ({e})"), format!("{host}: address lookup failed ({e})")))?
        .collect();
    if addrs.is_empty() {
        return Err(tr_s!(format!("{host}: 주소를 찾을 수 없습니다"), format!("{host}: no address found")));
    }
    addrs.sort_by_key(|a| match a {
        SocketAddr::V4(_) => 0,
        SocketAddr::V6(_) => 1,
    });
    let mut last = String::new();
    for a in addrs {
        match tokio::time::timeout(Duration::from_secs(5), tokio::net::TcpStream::connect(a)).await {
            Ok(Ok(s)) => {
                let _ = s.set_nodelay(true);
                return Ok(s);
            }
            Ok(Err(e)) => last = format!("{a}: {e}"),
            Err(_) => last = tr_s!(format!("{a}: 응답 없음"), format!("{a}: no response")),
        }
    }
    Err(last)
}

fn ssh_config() -> Arc<client::Config> {
    Arc::new(client::Config {
        keepalive_interval: Some(Duration::from_secs(30)),
        ..Default::default()
    })
}

/// SSH 핸드셰이크. 시간 제한은 호스트 키 확인 창이 떠 있는 동안 멈춘다.
async fn handshake(
    stream: tokio::net::TcpStream,
    cl: Client,
    limit: Duration,
) -> Result<SshHandle, String> {
    let (host, port) = (cl.host.clone(), cl.port);
    let prompting = cl.prompting.clone();
    let fut = client::connect_stream(ssh_config(), stream, cl);
    tokio::pin!(fut);
    let step = Duration::from_millis(250);
    let mut waited = Duration::ZERO;
    loop {
        tokio::select! {
            r = &mut fut => {
                return r.map_err(|e| match e {
                    russh::Error::UnknownKey => tr_s!(
                        format!("{host}:{port} 호스트 키를 신뢰하지 않아 접속을 중단했습니다"),
                        format!("{host}:{port} host key was not trusted; connection aborted")
                    ),
                    e => tr_s!(format!("연결 실패: {e}"), format!("connection failed: {e}")),
                });
            }
            _ = tokio::time::sleep(step) => {
                if !prompting.load(Ordering::SeqCst) {
                    waited += step;
                }
                if waited >= limit {
                    let secs = limit.as_secs();
                    return Err(tr_s!(
                        format!("{host}:{port} SSH 핸드셰이크 시간 초과 ({secs}초)"),
                        format!("{host}:{port} SSH handshake timed out ({secs}s)")
                    ));
                }
            }
        }
    }
}

// ---- 인증 (iDOT에서 이식 — 프롬프트만 대화상자로) -----------------------------

/// password → keyboard-interactive 순서로 인증 시도.
async fn try_password(handle: &mut SshHandle, user: &str, pass: &str) -> Result<bool, russh::Error> {
    match handle.authenticate_password(user, pass).await? {
        AuthResult::Success => return Ok(true),
        AuthResult::Failure { .. } => {}
    }
    let mut resp = handle
        .authenticate_keyboard_interactive_start(user, None::<String>)
        .await?;
    for _ in 0..10 {
        match resp {
            KeyboardInteractiveAuthResponse::Success => return Ok(true),
            KeyboardInteractiveAuthResponse::Failure { .. } => return Ok(false),
            KeyboardInteractiveAuthResponse::InfoRequest { prompts, .. } => {
                let answers: Vec<String> = prompts.iter().map(|_| pass.to_string()).collect();
                resp = handle
                    .authenticate_keyboard_interactive_respond(answers)
                    .await?;
            }
        }
    }
    Ok(false)
}

/// 개인키 파일을 읽는다. 암호 걸린 키면 저장된 비밀번호 → 대화상자(3회) 순으로 푼다.
async fn load_key(
    app: &AppHandle,
    path: &str,
    password: &str,
) -> Result<Option<russh::keys::PrivateKey>, String> {
    let path = path.trim().trim_matches('"').to_string();
    let text = match std::fs::read_to_string(&path) {
        Ok(t) => t,
        Err(e) => {
            return Err(tr_s!(
                format!("개인키 파일을 열 수 없습니다: {path} ({e})"),
                format!("cannot open private key file: {path} ({e})")
            ))
        }
    };
    let first = russh::keys::decode_secret_key(&text, None);
    let encrypted = match &first {
        Ok(k) => return Ok(Some(k.clone())),
        Err(russh::keys::Error::KeyIsEncrypted) => true,
        Err(_) => {
            text.contains("ENCRYPTED")
                || (text.starts_with("PuTTY-User-Key-File-") && !text.contains("Encryption: none"))
        }
    };
    if !encrypted {
        let e = first.err().map(|e| e.to_string()).unwrap_or_default();
        return Err(tr_s!(
            format!("개인키 형식을 읽을 수 없습니다: {path} ({e})"),
            format!("unsupported private key: {path} ({e})")
        ));
    }
    if !password.is_empty() {
        if let Ok(k) = russh::keys::decode_secret_key(&text, Some(password)) {
            return Ok(Some(k));
        }
    }
    for _ in 0..3 {
        let Some(pass) = prompt(app, "passphrase", &path, false).await else {
            return Err(cancelled());
        };
        if pass.is_empty() {
            break; // 빈 입력 = 키 인증 건너뛰기
        }
        if let Ok(k) = russh::keys::decode_secret_key(&text, Some(&pass)) {
            return Ok(Some(k));
        }
    }
    Ok(None)
}

async fn rsa_hash(handle: &SshHandle) -> Option<russh::keys::HashAlg> {
    tokio::time::timeout(Duration::from_secs(5), handle.best_supported_rsa_hash())
        .await
        .ok()
        .and_then(|r| r.ok())
        .flatten()
        .flatten()
}

/// 에이전트(Pageant / OpenSSH)에 들어 있는 키로 차례로 인증을 시도한다.
async fn try_agent<S>(
    handle: &mut SshHandle,
    user: &str,
    mut agent: russh::keys::agent::client::AgentClient<S>,
) -> Result<bool, String>
where
    S: russh::keys::agent::client::AgentStream + Send + Unpin + 'static,
{
    let ids = agent.request_identities().await.map_err(|e| e.to_string())?;
    for ident in ids.into_iter().take(8) {
        let key = ident.public_key().into_owned();
        let hash = if key.algorithm().is_rsa() { rsa_hash(handle).await } else { None };
        match handle.authenticate_publickey_with(user, key, hash, &mut agent).await {
            Ok(AuthResult::Success) => return Ok(true),
            Ok(AuthResult::Failure { .. }) => {}
            Err(e) => return Err(e.to_string()),
        }
    }
    Ok(false)
}

async fn agent_auth(handle: &mut SshHandle, user: &str) -> Result<bool, String> {
    use russh::keys::agent::client::AgentClient;
    #[cfg(windows)]
    {
        let mut found = false;
        if let Ok(a) = AgentClient::connect_pageant().await {
            found = true;
            if try_agent(handle, user, a).await.unwrap_or(false) {
                return Ok(true);
            }
        }
        if let Ok(a) = AgentClient::connect_named_pipe(r"\\.\pipe\openssh-ssh-agent").await {
            found = true;
            if try_agent(handle, user, a).await.unwrap_or(false) {
                return Ok(true);
            }
        }
        if !found {
            return Err(tr_s!(
                "실행 중인 Pageant / OpenSSH 에이전트가 없습니다",
                "no running Pageant / OpenSSH agent"
            ));
        }
        Ok(false)
    }
    #[cfg(not(windows))]
    {
        let a = AgentClient::connect_env().await.map_err(|e| e.to_string())?;
        try_agent(handle, user, a).await
    }
}

/// 접속 + 인증까지 한 번에. 성공 시 (핸들, 실제 사용자명)을 돌려준다.
/// `id`는 호스트키/인증 이벤트 상관관계용 접속 시도 식별자.
pub async fn connect_and_auth(
    app: &AppHandle,
    id: &str,
    hop: &Hop,
) -> Result<(SshHandle, String), String> {
    let socket = tcp_connect(&hop.host, hop.port).await?;
    let cl = Client {
        app: app.clone(),
        id: id.to_string(),
        host: hop.host.clone(),
        port: hop.port,
        prompting: Arc::new(AtomicBool::new(false)),
    };
    let mut handle = handshake(socket, cl, Duration::from_secs(15)).await?;

    let auth_err =
        |e: russh::Error| tr_s!(format!("인증 오류: {e}"), format!("authentication error: {e}"));
    let user = if hop.username.is_empty() {
        prompt(app, "user", &format!("{}:{}", hop.host, hop.port), true)
            .await
            .ok_or_else(cancelled)?
    } else {
        hop.username.clone()
    };

    if !hop.key_file.trim().is_empty() {
        match load_key(app, &hop.key_file, &hop.password).await {
            Ok(Some(key)) => {
                let hash = if key.algorithm().is_rsa() { rsa_hash(&handle).await } else { None };
                let key = PrivateKeyWithHashAlg::new(Arc::new(key), hash);
                match handle.authenticate_publickey(&user, key).await.map_err(auth_err)? {
                    AuthResult::Success => return Ok((handle, user)),
                    AuthResult::Failure { .. } => {}
                }
            }
            Ok(None) => {}
            Err(e) if e == cancelled() => return Err(e),
            Err(_) => {} // 키를 못 읽어도 비밀번호 인증으로 진행
        }
    }

    if hop.use_agent {
        if let Ok(true) = agent_auth(&mut handle, &user).await {
            return Ok((handle, user));
        }
    }

    if !hop.password.is_empty() {
        if try_password(&mut handle, &user, &hop.password).await.map_err(auth_err)? {
            return Ok((handle, user));
        }
    } else {
        for _ in 0..3 {
            let Some(pass) =
                prompt(app, "password", &format!("{user}@{}", hop.host), false).await
            else {
                return Err(cancelled());
            };
            if try_password(&mut handle, &user, &pass).await.map_err(auth_err)? {
                return Ok((handle, user));
            }
        }
    }
    Err(tr_s!(
        format!("인증 실패 ({}): 사용자명·비밀번호·키를 확인하세요", hop.host),
        format!("authentication failed ({}): check the user name, password or key", hop.host)
    ))
}

/// 서버 셸의 문자 집합을 묻는다 (locale charmap / LANG). EUC-KR이면 Some(true).
/// 파일 이름은 서버 로케일로 저장해야 서버의 ls 등에서 제대로 보인다. [iDOT에서 그대로]
pub async fn server_charset_is_euc(h: &SshHandle) -> Option<bool> {
    let probe = async {
        let mut ch = h.channel_open_session().await.ok()?;
        ch.exec(true, "locale charmap 2>/dev/null; echo \"|$LC_ALL|$LC_CTYPE|$LANG\"")
            .await
            .ok()?;
        let mut out = Vec::new();
        while let Some(m) = ch.wait().await {
            match m {
                russh::ChannelMsg::Data { data } => out.extend_from_slice(&data),
                russh::ChannelMsg::Eof | russh::ChannelMsg::Close => break,
                _ => {}
            }
        }
        Some(String::from_utf8_lossy(&out).to_ascii_uppercase())
    };
    let text = tokio::time::timeout(Duration::from_secs(4), probe).await.ok()??;
    let is_euc = |t: &str| ["EUC-KR", "EUCKR", "CP949", "UHC"].iter().any(|k| t.contains(k));
    let is_utf8 = |t: &str| t.contains("UTF-8") || t.contains("UTF8");
    for line in text.lines() {
        if is_euc(line) {
            return Some(true);
        }
        if is_utf8(line) {
            return Some(false);
        }
    }
    None
}
