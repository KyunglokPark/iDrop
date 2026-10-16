// 자동 재연결의 핵심 보장 검증:
//   부분 전송 → "연결 완전 손실"(SSH 핸들 drop) → 새 핸들로 재접속 →
//   공유 NameCodec 유지한 채 offset부터 이어받기 → 해시 일치.
//
// iDrop SitePool::reconnect_once()가 하는 일(죽은 핸들 폐기 → 새 핸들 → 같은 codec)을
// 그대로 모사한다. Tauri에 묶인 pool/queue 오케스트레이션 대신, 그 토대가 되는
// "전송이 연결 손실을 넘어 새 연결에서 이어진다"는 불변식을 실제 OpenSSH 대상으로 검증.

use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;

use idrop_sftp_e2e::{connect, open_sftp, sftp};
use sha2::{Digest, Sha256};

fn hash(bytes: &[u8]) -> Vec<u8> {
    let mut h = Sha256::new();
    h.update(bytes);
    h.finalize().to_vec()
}

#[tokio::main]
async fn main() {
    // 사이트 하나가 쓰는 공유 codec (재연결해도 유지되는 것 모사)
    let codec = Arc::new(sftp::NameCodec::default());
    let cancel = AtomicBool::new(false);

    // 테스트 파일 (1.5MB — 채널 윈도 경계 넘김), 한글 이름 포함
    let data: Vec<u8> = (0..1_500_000u32).map(|i| (i.wrapping_mul(2654435761) >> 11) as u8).collect();
    let h_full = hash(&data);
    let half = data.len() as u64 / 2;

    let tmp = std::env::temp_dir();
    let src = tmp.join("idrop-reconn-src.bin");
    std::fs::write(&src, &data).unwrap();

    // ---- 1) 핸들1로 접속 → 홈 아래 작업 디렉터리 준비 ----
    let base;
    {
        let h1 = connect(2222).await;
        let s1 = open_sftp(&h1, codec.clone()).await;
        let home = s1.realpath(".").await.expect("realpath");
        base = format!("{home}/idrop-e2e-reconnect");
        let _ = s1.remove(&base).await;
        s1.mkdir(&base).await.expect("mkdir");
        // 앞 절반만 업로드 (전송 중 끊긴 상태 모사)
        let partial = tmp.join("idrop-reconn-half.bin");
        std::fs::write(&partial, &data[..half as usize]).unwrap();
        let remote = format!("{base}/재연결 테스트.bin");
        s1.upload_file(&partial, &remote, 0, &cancel, &mut |_| {}).await.expect("partial up");
        assert_eq!(s1.stat(&remote, true).await.unwrap().size, Some(half), "부분 업로드 크기");
        // ---- 연결 완전 손실: 핸들1과 그 채널들을 전부 drop ----
        drop(s1);
        drop(h1);
    }

    let remote = format!("{base}/재연결 테스트.bin");

    // ---- 2) 핸들2로 재접속 (reconnect_once 모사) → 같은 codec으로 이어받기 ----
    let h2 = connect(2222).await;
    let s2 = open_sftp(&h2, codec.clone()).await;
    // 업로드 이어받기: half부터 끝까지
    let mut up2 = 0u64;
    s2.upload_file(&src, &remote, half, &cancel, &mut |d| up2 += d).await.expect("resume up");
    assert_eq!(up2, data.len() as u64 - half, "이어받은 바이트=나머지");

    // 다운로드로 원격 전체 해시 검증
    let back: PathBuf = tmp.join("idrop-reconn-back.bin");
    let _ = std::fs::remove_file(&back);
    s2.download_file(&remote, &back, 0, &cancel, &mut |_| {}).await.expect("verify dl");
    assert_eq!(h_full, hash(&std::fs::read(&back).unwrap()), "업로드 이어받기(재연결) 해시 불일치");

    // ---- 3) 다운로드 이어받기도 재연결을 넘어 동작하는지 ----
    // 로컬에 앞 절반만 받은 상태 → 핸들2를 drop → 핸들3로 재접속 → offset부터 재개
    let rdl = tmp.join("idrop-reconn-dl.bin");
    std::fs::write(&rdl, &data[..half as usize]).unwrap();
    drop(s2);
    drop(h2);

    let h3 = connect(2222).await;
    let s3 = open_sftp(&h3, codec.clone()).await;
    let mut dn2 = 0u64;
    s3.download_file(&remote, &rdl, half, &cancel, &mut |d| dn2 += d).await.expect("resume dl");
    assert_eq!(dn2, data.len() as u64 - half, "이어받은 바이트=나머지");
    assert_eq!(h_full, hash(&std::fs::read(&rdl).unwrap()), "다운로드 이어받기(재연결) 해시 불일치");

    // 정리
    s3.remove(&base).await.expect("cleanup");

    println!("PASS reconnect: 연결 완전 손실 후 새 핸들로 업/다운 이어받기 해시 일치 (공유 codec 유지)");
}
