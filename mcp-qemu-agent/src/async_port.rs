use std::fs::{File, OpenOptions};
use std::io::{Read, Write};
use std::os::unix::prelude::OpenOptionsExt;
use std::path::Path;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll, ready};

use tokio::io::unix::AsyncFd;
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};

////////////////////////////////////////////////////////////////////////////////
/// WARNING: not `tokio::fs::File`. That one runs every operation on a blocking thread and
/// tracks it in one state machine shared by both directions, so `tokio::io::split` does not
/// give two independent halves: write cannot start while a read is in flight, and a read
/// of this port is in flight until the host sends something. The agent would answer every
/// frame only once the next one arrived, which to the host looks like an agent that never
/// answers at all. An `AsyncFd` polls the device, where the two readiness are separate.
#[derive(Clone)]
pub struct AsyncPort {
    port: Arc<AsyncFd<File>>,
}

impl AsyncPort {
    ////////////////////////////////////////////////////////////////////////////////
    pub fn open(path: impl AsRef<Path>) -> std::io::Result<Self> {
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .custom_flags(libc::O_NONBLOCK)
            .open(path)?;

        Ok(Self {
            port: Arc::new(AsyncFd::new(file)?),
        })
    }
}

impl AsyncRead for AsyncPort {
    ////////////////////////////////////////////////////////////////////////////////
    fn poll_read(self: Pin<&mut Self>, cx: &mut Context<'_>, dst: &mut ReadBuf<'_>) -> Poll<std::io::Result<()>> {
        let this = self.get_mut();

        loop {
            let mut guard = ready!(this.port.poll_read_ready(cx))?;

            // WARNING: readiness can be a false alarm, so a would-block here is not an error,
            // it is a `clear_ready` and another wait
            match guard.try_io(|inner| {
                let mut file: &File = inner.get_ref();
                file.read(dst.initialize_unfilled())
            }) {
                Ok(Ok(count)) => {
                    dst.advance(count);
                    return Poll::Ready(Ok(()));
                }
                Ok(Err(e)) => return Poll::Ready(Err(e)),
                Err(_would_block) => continue,
            }
        }
    }
}

impl AsyncWrite for AsyncPort {
    ////////////////////////////////////////////////////////////////////////////////
    fn poll_write(self: Pin<&mut Self>, cx: &mut Context<'_>, src: &[u8]) -> Poll<std::io::Result<usize>> {
        let this = self.get_mut();

        loop {
            let mut guard = ready!(this.port.poll_write_ready(cx))?;

            match guard.try_io(|inner| {
                let mut file: &File = inner.get_ref();
                file.write(src)
            }) {
                Ok(result) => return Poll::Ready(result),
                Err(_would_block) => continue,
            }
        }
    }

    ////////////////////////////////////////////////////////////////////////////////
    fn poll_flush(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        // nothing of ours is buffered: every write above went straight to the device
        Poll::Ready(Ok(()))
    }

    ////////////////////////////////////////////////////////////////////////////////
    fn poll_shutdown(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Poll::Ready(Ok(()))
    }
}
