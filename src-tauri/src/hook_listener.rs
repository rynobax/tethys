use std::path::Path;
use std::sync::Arc;

use tokio::io::AsyncReadExt;
use tokio::net::{UnixListener, UnixStream};
use tracing::{debug, error, info, warn};

use crate::error::AppResult;
use crate::sessions::SessionSupervisor;

/// Payload sent by `tethys-hook` over the UDS. Defined once in the companion
/// crate so the sender can't grow a field the receiver silently drops.
pub use tethys_hook::HookMessage;

/// Bind `hook.sock` and spawn an accept loop. If the socket already exists
/// (prior run crashed without cleanup), remove it first.
pub async fn start(
    socket_path: &Path,
    supervisor: Arc<SessionSupervisor>,
) -> AppResult<()> {
    if socket_path.exists() {
        tokio::fs::remove_file(socket_path).await.ok();
    }
    if let Some(parent) = socket_path.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }

    let listener = UnixListener::bind(socket_path)?;
    info!(path = %socket_path.display(), "hook socket listening");

    tokio::spawn(async move {
        loop {
            match listener.accept().await {
                Ok((stream, _)) => {
                    let sup = supervisor.clone();
                    tokio::spawn(async move {
                        if let Err(e) = handle_connection(stream, sup).await {
                            warn!(error = %e, "hook connection error");
                        }
                    });
                }
                Err(e) => {
                    error!(error = %e, "hook accept failed");
                }
            }
        }
    });

    Ok(())
}

const MAX_FRAME_BYTES: usize = 1024 * 1024;

async fn handle_connection(
    mut stream: UnixStream,
    supervisor: Arc<SessionSupervisor>,
) -> AppResult<()> {
    let mut len_buf = [0u8; 4];
    stream.read_exact(&mut len_buf).await?;
    let len = u32::from_be_bytes(len_buf) as usize;
    if len == 0 || len > MAX_FRAME_BYTES {
        warn!(len, "hook frame length out of bounds");
        return Ok(());
    }

    let mut buf = vec![0u8; len];
    stream.read_exact(&mut buf).await?;

    let msg: HookMessage = match serde_json::from_slice(&buf) {
        Ok(m) => m,
        Err(e) => {
            warn!(error = %e, "hook message parse failed");
            return Ok(());
        }
    };

    debug!(event = %msg.event, token = ?msg.spawn_token, "hook event received");
    supervisor.handle_hook_event(msg).await;
    Ok(())
}
