// 왕복 검증: 업로드 → 다운로드 해시 일치 → rename → 재귀 삭제.
// 대상: WSL sshd(127.0.0.1:2222)의 실제 OpenSSH sftp-server.

use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;

use idrop_sftp_e2e::{connect, open_sftp, sftp};
use sha2::{Digest, Sha256};

fn hash(path: &std::path::Path) -> Vec<u8> {
    let mut h = Sha256::new();
    h.update(std::fs::read(path).unwrap());
    h.finalize().to_vec()
}

#[tokio::main]
async fn main() {
    let h = connect(2222).await;
    let codec = Arc::new(sftp::NameCodec::default());
    let s = open_sftp(&h, codec).await;

    let home = s.realpath(".").await.expect("realpath");
    let base = format!("{home}/idrop-e2e-roundtrip");
    let _ = s.remove(&base).await; // 이전 실행 잔재 정리
    s.mkdir(&base).await.expect("mkdir");

    // 1) 로컬 테스트 파일 (1.5MB — CHUNK×WINDOW 경계를 넘긴다)
    let tmp = std::env::temp_dir().join("idrop-e2e-src.bin");
    let data: Vec<u8> = (0..1_500_000u32).map(|i| (i.wrapping_mul(2654435761) >> 13) as u8).collect();
    std::fs::write(&tmp, &data).unwrap();

    // 2) 업로드
    let cancel = AtomicBool::new(false);
    let remote = format!("{base}/파일 테스트.bin"); // 한글 이름 포함
    let mut total = 0u64;
    s.upload_file(&tmp, &remote, 0, &cancel, &mut |delta| total += delta)
        .await
        .expect("upload");
    assert_eq!(total, data.len() as u64, "업로드 진행률 증분 합계");

    // 3) 다운로드 후 해시 비교
    let dl: PathBuf = std::env::temp_dir().join("idrop-e2e-dl.bin");
    let _ = std::fs::remove_file(&dl);
    s.download_file(&remote, &dl, 0, &cancel, &mut |_| {})
        .await
        .expect("download");
    assert_eq!(hash(&tmp), hash(&dl), "왕복 해시 불일치!");

    // 3b) 이어받기(offset) — 실제 OpenSSH sftp-server 대상 SFTP 재개 검증
    let half = data.len() as u64 / 2;
    // 업로드 이어받기: 원격에 앞 절반만 올린 뒤 offset부터 재개
    let rremote = format!("{base}/resume.bin");
    {
        let partial = std::env::temp_dir().join("idrop-e2e-partial.bin");
        std::fs::write(&partial, &data[..half as usize]).unwrap();
        s.upload_file(&partial, &rremote, 0, &cancel, &mut |_| {}).await.expect("partial up");
        assert_eq!(s.stat(&rremote, true).await.unwrap().size, Some(half), "부분 업로드 크기");
        let mut up2 = 0u64;
        s.upload_file(&tmp, &rremote, half, &cancel, &mut |d| up2 += d).await.expect("resume up");
        assert_eq!(up2, data.len() as u64 - half, "이어받은 바이트=나머지");
        let back = std::env::temp_dir().join("idrop-e2e-resume-back.bin");
        let _ = std::fs::remove_file(&back);
        s.download_file(&rremote, &back, 0, &cancel, &mut |_| {}).await.expect("verify dl");
        assert_eq!(hash(&tmp), hash(&back), "업로드 이어받기 해시 불일치!");
    }
    // 다운로드 이어받기: 로컬에 앞 절반만 받은 상태에서 offset부터 재개
    {
        let rdl = std::env::temp_dir().join("idrop-e2e-resume-dl.bin");
        std::fs::write(&rdl, &data[..half as usize]).unwrap();
        let mut dn2 = 0u64;
        s.download_file(&remote, &rdl, half, &cancel, &mut |d| dn2 += d).await.expect("resume dl");
        assert_eq!(dn2, data.len() as u64 - half, "이어받은 바이트=나머지");
        assert_eq!(hash(&tmp), hash(&rdl), "다운로드 이어받기 해시 불일치!");
    }

    // 4) rename → stat → 재귀 삭제
    let renamed = format!("{base}/renamed.bin");
    s.rename(&remote, &renamed).await.expect("rename");
    let st = s.stat(&renamed, true).await.expect("stat");
    assert_eq!(st.size, Some(data.len() as u64));
    s.remove(&base).await.expect("recursive remove");
    assert!(s.stat(&base, true).await.is_err(), "삭제됐어야 함");

    println!("PASS roundtrip: 업로드/다운로드 해시 일치 + 이어받기(업/다운) + rename + 재귀 삭제");
}
