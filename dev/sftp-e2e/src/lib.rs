// iDrop SFTP e2e 공용부 — 실제 sftp.rs를 #[path]로 포함 (드리프트 없음).
//
// 인증: 테스트 전용 ed25519 키를 만들어 ~/.ssh/authorized_keys에 등록해 쓴다
// (비밀번호를 몰라도 WSL sshd에 접속 가능). 실제 앱의 인증 경로(password/KI/키/에이전트)는
// iDOT에서 검증된 코드를 그대로 쓰므로 여기서는 SFTP 프로토콜·NameCodec·다중 채널에 집중한다.

#[path = "../../../src-tauri/src/sftp.rs"]
pub mod sftp;

use std::sync::Arc;
use std::time::Duration;

use russh::client::{self, AuthResult};
use russh::keys::{PrivateKeyWithHashAlg, PublicKeyOrCertificate};
use sftp::{NameCodec, Sftp};

pub struct Accept;

impl client::Handler for Accept {
    type Error = russh::Error;
    async fn check_server_key(&mut self, _k: &PublicKeyOrCertificate) -> Result<bool, Self::Error> {
        Ok(true)
    }
}

pub type Handle = client::Handle<Accept>;

/// 테스트 키를 보장한다: ~/.ssh/idrop_e2e_ed25519 (없으면 생성 + authorized_keys에 등록)
pub fn ensure_test_key() -> russh::keys::PrivateKey {
    let home = std::env::var("HOME").expect("HOME");
    let dir = std::path::Path::new(&home).join(".ssh");
    std::fs::create_dir_all(&dir).unwrap();
    let key_path = dir.join("idrop_e2e_ed25519");
    if let Ok(text) = std::fs::read_to_string(&key_path) {
        if let Ok(k) = russh::keys::decode_secret_key(&text, None) {
            return k;
        }
    }
    let key = russh::keys::PrivateKey::random(&mut rand::rng(), russh::keys::Algorithm::Ed25519)
        .expect("keygen");
    let pem = key.to_openssh(russh::keys::ssh_key::LineEnding::LF).expect("pem");
    std::fs::write(&key_path, pem.as_bytes()).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&key_path, std::fs::Permissions::from_mode(0o600));
    }
    let pubkey = key.public_key().to_openssh().expect("pub");
    let auth = dir.join("authorized_keys");
    let mut lines = std::fs::read_to_string(&auth).unwrap_or_default();
    if !lines.contains(&pubkey) {
        if !lines.is_empty() && !lines.ends_with('\n') {
            lines.push('\n');
        }
        lines.push_str(&format!("{pubkey} idrop-e2e\n"));
        std::fs::write(&auth, lines).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&auth, std::fs::Permissions::from_mode(0o600));
        }
    }
    key
}

/// 127.0.0.1:port 의 sshd에 현재 사용자 + 테스트 키로 접속한다.
pub async fn connect(port: u16) -> Handle {
    let key = ensure_test_key();
    let user = std::env::var("USER").expect("USER");
    let config = Arc::new(client::Config::default());
    let mut h = client::connect(config, ("127.0.0.1", port), Accept)
        .await
        .expect("sshd 접속 실패 — WSL sshd(2222)가 떠 있는지 확인");
    let auth = h
        .authenticate_publickey(&user, PrivateKeyWithHashAlg::new(Arc::new(key), None))
        .await
        .expect("인증 요청 실패");
    assert!(matches!(auth, AuthResult::Success), "공개키 인증 실패 — authorized_keys 확인");
    h
}

/// SFTP 채널을 연다 (공유 codec 주입 — iDrop pool.rs와 같은 방식)
pub async fn open_sftp(h: &Handle, codec: Arc<NameCodec>) -> Arc<Sftp> {
    let ch = h.channel_open_session().await.expect("channel");
    ch.request_subsystem(true, "sftp").await.expect("subsystem");
    tokio::time::timeout(Duration::from_secs(10), Sftp::start(ch.into_stream(), codec))
        .await
        .expect("SFTP 응답 없음")
        .expect("SFTP INIT 실패")
}
