use std::{
    io,
    pin::Pin,
    process::Stdio,
    task::{Context, Poll, ready},
};

use rmcp::{
    RoleClient,
    service::{RxJsonRpcMessage, TxJsonRpcMessage},
    transport::{Transport, async_rw::AsyncRwTransport},
};
use tokio::{
    io::{AsyncRead, ReadBuf},
    process::{Child, ChildStdin, ChildStdout, Command},
};

// A maximally JSON-escaped 16 MiB string still fits below this wire limit.
const MAX_MESSAGE_BYTES: usize = 128 * 1024 * 1024;

struct BoundedLines<R> {
    inner: R,
    maximum: usize,
    current: usize,
    failed: bool,
}

impl<R> BoundedLines<R> {
    fn new(inner: R, maximum: usize) -> Self {
        Self {
            inner,
            maximum,
            current: 0,
            failed: false,
        }
    }

    fn oversized() -> io::Error {
        io::Error::new(
            io::ErrorKind::InvalidData,
            "MCP message exceeds wire byte limit",
        )
    }
}

impl<R: AsyncRead + Unpin> AsyncRead for BoundedLines<R> {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        let this = self.get_mut();
        if this.failed {
            return Poll::Ready(Err(Self::oversized()));
        }
        let start = buf.filled().len();
        ready!(Pin::new(&mut this.inner).poll_read(cx, buf))?;
        for byte in &buf.filled()[start..] {
            if this.current == this.maximum {
                this.failed = true;
                return Poll::Ready(Err(Self::oversized()));
            }
            this.current += 1;
            if *byte == b'\n' {
                this.current = 0;
            }
        }
        Poll::Ready(Ok(()))
    }
}

pub(crate) struct ChildTransport {
    transport: AsyncRwTransport<RoleClient, BoundedLines<ChildStdout>, ChildStdin>,
    child: Option<Child>,
}

impl ChildTransport {
    pub(crate) fn spawn(mut command: Command) -> io::Result<Self> {
        let mut child = command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .kill_on_drop(true)
            .spawn()?;
        let stdin = child.stdin.take().expect("piped child stdin");
        let stdout = child.stdout.take().expect("piped child stdout");
        Ok(Self {
            transport: AsyncRwTransport::new(BoundedLines::new(stdout, MAX_MESSAGE_BYTES), stdin)
                .with_max_line_length(MAX_MESSAGE_BYTES),
            child: Some(child),
        })
    }
}

impl Transport<RoleClient> for ChildTransport {
    type Error = io::Error;

    fn send(
        &mut self,
        item: TxJsonRpcMessage<RoleClient>,
    ) -> impl Future<Output = Result<(), Self::Error>> + Send + 'static {
        self.transport.send(item)
    }

    async fn receive(&mut self) -> Option<RxJsonRpcMessage<RoleClient>> {
        self.transport.receive().await
    }

    async fn close(&mut self) -> Result<(), Self::Error> {
        // SDK sends flush each message; close drops stdin so the child observes EOF.
        self.transport.close().await?;
        if let Some(child) = self.child.as_mut() {
            child.wait().await?;
            self.child.take();
        }
        Ok(())
    }
}

impl Drop for ChildTransport {
    fn drop(&mut self) {
        // kill_on_drop remains the fallback when the runtime has already stopped.
        if let Some(mut child) = self.child.take()
            && let Ok(runtime) = tokio::runtime::Handle::try_current()
        {
            runtime.spawn(async move {
                if let Err(error) = child.kill().await {
                    tracing::warn!(%error, "downstream child cleanup failed");
                }
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, BufReader};

    #[tokio::test]
    async fn bounded_lines_counts_across_reads_and_resets_at_newlines() {
        let input = BufReader::with_capacity(2, &b"abc\ndef\nghi"[..]);
        let mut reader = BoundedLines::new(input, 4);
        for expected in b"abc\ndef\nghi" {
            assert_eq!(reader.read_u8().await.unwrap(), *expected);
        }
        assert_eq!(reader.read(&mut [0]).await.unwrap(), 0);
    }

    #[tokio::test]
    async fn bounded_lines_fails_closed_without_waiting_for_newline() {
        let (input, mut output) = tokio::io::duplex(16);
        use tokio::io::AsyncWriteExt;
        output.write_all(b"12345").await.unwrap();
        let mut reader = BoundedLines::new(input, 4);
        let mut bytes = [0; 4];
        reader.read_exact(&mut bytes).await.unwrap();
        let error = reader.read_u8().await.unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
        assert_eq!(reader.read_u8().await.unwrap_err().kind(), error.kind());
    }

    #[tokio::test]
    async fn oversized_line_closes_sdk_transport_instead_of_discarding_response() {
        let input = BoundedLines::new(&b"12345"[..], 4);
        let mut transport = AsyncRwTransport::<RoleClient, _, _>::new(input, tokio::io::sink())
            .with_max_line_length(4);
        assert!(transport.receive().await.is_none());
    }

    #[tokio::test]
    async fn sdk_decodes_messages_larger_than_its_default_line_cap() {
        let mut bytes = br#"{"jsonrpc":"2.0","method":"notifications/message","params":{"level":"info","data":""#.to_vec();
        bytes.extend(std::iter::repeat_n(b'x', 17 * 1024 * 1024));
        bytes.extend_from_slice(b"\"}}\n");
        let input = BoundedLines::new(bytes.as_slice(), MAX_MESSAGE_BYTES);
        let mut transport = AsyncRwTransport::<RoleClient, _, _>::new(input, tokio::io::sink())
            .with_max_line_length(MAX_MESSAGE_BYTES);
        assert!(transport.receive().await.is_some());
    }

    #[cfg(unix)]
    #[tokio::test(start_paused = true)]
    async fn child_shutdown_has_no_wall_clock_deadline() {
        let mut command = Command::new("sh");
        command.args(["-c", "read -r line; kill -STOP $$"]);
        let mut transport = ChildTransport::spawn(command).unwrap();
        let id = transport.child.as_ref().unwrap().id().unwrap();
        let mut close = Box::pin(transport.close());
        assert!(futures_util::poll!(&mut close).is_pending());
        tokio::time::advance(std::time::Duration::from_secs(24 * 60 * 60)).await;
        assert!(futures_util::poll!(&mut close).is_pending());
        assert!(
            std::process::Command::new("kill")
                .args(["-KILL", &id.to_string()])
                .status()
                .unwrap()
                .success()
        );
        close.await.unwrap();
        assert!(transport.child.is_none());
    }
}
