use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

use crate::IpcError;

/// Maximum payload size accepted by framing helpers.
pub const MAX_FRAME_SIZE: u32 = 8 * 1024 * 1024;

/// Writes one length-prefixed frame to the async writer.
pub async fn write_frame<W: AsyncWrite + Unpin>(
    writer: &mut W,
    payload: &[u8],
) -> Result<(), IpcError> {
    let size: u32 = payload
        .len()
        .try_into()
        .map_err(|_| IpcError::FrameTooLarge {
            size: u32::MAX,
            max: MAX_FRAME_SIZE,
        })?;

    if size > MAX_FRAME_SIZE {
        return Err(IpcError::FrameTooLarge {
            size,
            max: MAX_FRAME_SIZE,
        });
    }

    writer.write_all(&size.to_be_bytes()).await?;
    writer.write_all(payload).await?;
    writer.flush().await?;
    Ok(())
}

/// Reads one length-prefixed frame from the async reader.
pub async fn read_frame<R: AsyncRead + Unpin>(reader: &mut R) -> Result<Vec<u8>, IpcError> {
    let mut header = [0_u8; 4];
    reader.read_exact(&mut header).await?;

    let size = u32::from_be_bytes(header);
    if size > MAX_FRAME_SIZE {
        return Err(IpcError::FrameTooLarge {
            size,
            max: MAX_FRAME_SIZE,
        });
    }

    let mut payload = vec![0_u8; size as usize];
    reader.read_exact(&mut payload).await?;
    Ok(payload)
}
