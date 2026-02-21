use planter_ipc::{
    IpcError,
    framing::{MAX_FRAME_SIZE, read_frame, write_frame},
};
use tokio::io::{AsyncWriteExt, duplex, sink};

#[tokio::test]
async fn frame_roundtrip() {
    let (mut tx, mut rx) = duplex(128);
    let payload = b"hello-frame".to_vec();

    let write_task = tokio::spawn(async move { write_frame(&mut tx, &payload).await });
    let read_payload = read_frame(&mut rx).await.expect("read should succeed");

    write_task
        .await
        .expect("join should succeed")
        .expect("write should succeed");
    assert_eq!(read_payload, b"hello-frame");
}

#[tokio::test]
async fn reject_oversized_frame() {
    let mut writer = sink();
    let payload = vec![0_u8; (MAX_FRAME_SIZE + 1) as usize];

    let err = write_frame(&mut writer, &payload)
        .await
        .expect_err("oversized frame must fail");

    match err {
        IpcError::FrameTooLarge { .. } => {}
        other => panic!("unexpected error: {other}"),
    }
}

#[tokio::test]
async fn detect_truncated_frame_payload() {
    let (mut tx, mut rx) = duplex(128);

    tx.write_all(&(8_u32.to_be_bytes()))
        .await
        .expect("header write should succeed");
    tx.write_all(b"abc")
        .await
        .expect("partial payload write should succeed");
    drop(tx);

    let err = read_frame(&mut rx)
        .await
        .expect_err("truncated frame should fail");

    match err {
        IpcError::Io(io_err) => {
            assert_eq!(io_err.kind(), std::io::ErrorKind::UnexpectedEof);
        }
        other => panic!("unexpected error: {other}"),
    }
}
