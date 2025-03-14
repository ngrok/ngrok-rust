use std::{
    io,
    mem,
    pin::{
        pin,
        Pin,
    },
    task::{
        ready,
        Context,
        Poll,
    },
};

use bytes::{
    Buf,
    BytesMut,
};
use proxy_protocol::{
    ParseError,
    ProxyHeader,
};
use tokio::io::{
    AsyncRead,
    AsyncWrite,
    ReadBuf,
};
use tracing::instrument;

// 536 is the smallest possible TCP segment, which both v1 and v2 are guaranteed
// to fit into.
const MAX_HEADER_LEN: usize = 536;
// v2 headers start with at least 16 bytes
const MIN_HEADER_LEN: usize = 16;

#[derive(Debug)]
enum ReadState {
    Reading(Option<ParseError>, BytesMut),
    Error(proxy_protocol::ParseError, BytesMut),
    Header(Option<proxy_protocol::ProxyHeader>, BytesMut),
    None,
}

impl ReadState {
    fn new() -> ReadState {
        ReadState::Reading(None, BytesMut::with_capacity(MAX_HEADER_LEN))
    }

    fn header(&self) -> Result<Option<&ProxyHeader>, &ParseError> {
        match self {
            ReadState::Error(err, _) | ReadState::Reading(Some(err), _) => Err(err),
            ReadState::None | ReadState::Reading(None, _) => Ok(None),
            ReadState::Header(hdr, _) => Ok(hdr.as_ref()),
        }
    }

    /// Read the header from the stream *once*. Once a header has been read, or
    /// it's been determined that no header is coming, this will be a no-op.
    #[instrument(level = "trace", skip(reader))]
    fn poll_read_header_once(
        &mut self,
        cx: &mut Context,
        mut reader: Pin<&mut impl AsyncRead>,
    ) -> Poll<io::Result<()>> {
        loop {
            let read_state = mem::replace(self, ReadState::None);
            let (last_err, mut hdr_buf) = match read_state {
                // End states
                ReadState::None | ReadState::Header(_, _) | ReadState::Error(_, _) => {
                    *self = read_state;
                    return Poll::Ready(Ok(()));
                }
                ReadState::Reading(err, hdr_buf) => (err, hdr_buf),
            };

            if hdr_buf.len() < MAX_HEADER_LEN {
                let mut tmp_buf = ReadBuf::uninit(hdr_buf.spare_capacity_mut());
                let read_res = reader.as_mut().poll_read(cx, &mut tmp_buf);
                // Regardless of error, make sure we track the read bytes
                let filled = tmp_buf.filled().len();
                if filled > 0 {
                    let len = hdr_buf.len();
                    // Safety: the tmp_buf is backed by the uninitialized
                    // portion of hdr_buf. Advancing the len to len + filled is
                    // guaranteed to only cover the bytes initialized by the
                    // read.
                    unsafe { hdr_buf.set_len(len + filled) }
                }
                match read_res {
                    // If we hit the end of the stream due to either an EOF or
                    // an error, set the state to a terminal one and return the
                    // result.
                    Poll::Ready(ref res) if res.is_err() || filled == 0 => {
                        *self = match last_err {
                            Some(err) => ReadState::Error(err, hdr_buf),
                            None => ReadState::Header(None, hdr_buf),
                        };
                        return read_res;
                    }
                    // Pending leaves the last error and buffer unchanged.
                    Poll::Pending => {
                        *self = ReadState::Reading(last_err, hdr_buf);
                        return read_res;
                    }
                    _ => {}
                }
            }

            // Create a view into the header buffer so that failed parse
            // attempts don't consume it.
            let mut hdr_view = &*hdr_buf;

            // Don't try to parse unless we have a minimum number of bytes to
            // avoid spurious "NotProxyHeader" errors.
            // Also hack around a bug in the proxy_protocol crate that results
            // in panics when the input ends in \r without the \n.
            if hdr_view.len() < MIN_HEADER_LEN || matches!(hdr_view.last(), Some(b'\r')) {
                *self = ReadState::Reading(last_err, hdr_buf);
                continue;
            }

            match proxy_protocol::parse(&mut hdr_view) {
                Ok(hdr) => {
                    hdr_buf.advance(hdr_buf.len() - hdr_view.len());
                    *self = ReadState::Header(Some(hdr), hdr_buf);
                    return Poll::Ready(Ok(()));
                }
                Err(ParseError::NotProxyHeader) => {
                    *self = ReadState::Header(None, hdr_buf);
                    return Poll::Ready(Ok(()));
                }

                // Keep track of the last error - it might not be fatal if we
                // simply haven't read enough
                Err(err) => {
                    // If we've read too much, consider the error fatal.
                    if hdr_buf.len() >= MAX_HEADER_LEN {
                        *self = ReadState::Error(err, hdr_buf);
                    } else {
                        *self = ReadState::Reading(Some(err), hdr_buf);
                    }
                    continue;
                }
            }
        }
    }
}

#[derive(Debug)]
enum WriteState {
    Writing(BytesMut),
    None,
}

impl WriteState {
    fn new(hdr: proxy_protocol::ProxyHeader) -> Result<WriteState, proxy_protocol::EncodeError> {
        proxy_protocol::encode(hdr).map(WriteState::Writing)
    }

    /// Write the header *once*. After its written to the stream, this will be a
    /// no-op.
    #[instrument(level = "trace", skip(writer))]
    fn poll_write_header_once(
        &mut self,
        cx: &mut Context,
        mut writer: Pin<&mut impl AsyncWrite>,
    ) -> Poll<io::Result<()>> {
        loop {
            let state = mem::replace(self, WriteState::None);
            match state {
                WriteState::None => return Poll::Ready(Ok(())),
                WriteState::Writing(mut buf) => {
                    let write_res = writer.as_mut().poll_write(cx, &buf);
                    match write_res {
                        Poll::Pending | Poll::Ready(Err(_)) => {
                            *self = WriteState::Writing(buf);
                            ready!(write_res)?;
                            unreachable!(
                                "ready! will return for us on either Pending or Ready(Err)"
                            );
                        }
                        Poll::Ready(Ok(written)) => {
                            buf.advance(written);
                            if !buf.is_empty() {
                                *self = WriteState::Writing(buf);
                                continue;
                            } else {
                                return Ok(()).into();
                            }
                        }
                    }
                }
            }
        }
    }
}

#[derive(Debug)]
#[pin_project::pin_project]
pub struct Stream<S> {
    read_state: ReadState,
    write_state: WriteState,
    #[pin]
    inner: S,
}

impl<S> Stream<S> {
    pub fn outgoing(stream: S, header: ProxyHeader) -> Result<Self, proxy_protocol::EncodeError> {
        Ok(Stream {
            inner: stream,
            write_state: WriteState::new(header)?,
            read_state: ReadState::None,
        })
    }

    pub fn incoming(stream: S) -> Self {
        Stream {
            inner: stream,
            read_state: ReadState::new(),
            write_state: WriteState::None,
        }
    }

    pub fn disabled(stream: S) -> Self {
        Stream {
            inner: stream,
            read_state: ReadState::None,
            write_state: WriteState::None,
        }
    }
}

impl<S> Stream<S>
where
    S: AsyncRead,
{
    #[instrument(level = "trace", skip(self), fields(read_state = ?self.read_state))]
    pub fn poll_proxy_header(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<io::Result<Result<Option<&ProxyHeader>, &ParseError>>> {
        let this = self.project();

        ready!(this.read_state.poll_read_header_once(cx, this.inner))?;

        Ok(this.read_state.header()).into()
    }

    #[instrument(level = "debug", skip(self))]
    pub async fn proxy_header(&mut self) -> io::Result<Result<Option<&ProxyHeader>, &ParseError>>
    where
        Self: Unpin,
    {
        let mut this = Pin::new(self);

        futures::future::poll_fn(|cx| {
            let this = this.as_mut().project();
            this.read_state.poll_read_header_once(cx, this.inner)
        })
        .await?;

        Ok(this.get_mut().read_state.header())
    }
}

impl<S> AsyncRead for Stream<S>
where
    S: AsyncRead,
{
    #[instrument(level = "trace", skip(self), fields(read_state = ?self.read_state))]
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        let mut this = self.project();

        ready!(this
            .read_state
            .poll_read_header_once(cx, this.inner.as_mut()))?;

        match this.read_state {
            ReadState::Error(_, remainder) | ReadState::Header(_, remainder) => {
                if !remainder.is_empty() {
                    let available = std::cmp::min(remainder.len(), buf.remaining());
                    buf.put_slice(&remainder.split_to(available));
                    // Make sure Ready is returned regardless of inner's state
                    return Poll::Ready(Ok(()));
                }
            }
            ReadState::None => {}
            _ => unreachable!(),
        }

        this.inner.poll_read(cx, buf)
    }
}

impl<S> AsyncWrite for Stream<S>
where
    S: AsyncWrite,
{
    #[instrument(level = "trace", skip(self), fields(write_state = ?self.write_state))]
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<Result<usize, io::Error>> {
        let mut this = self.project();

        ready!(this
            .write_state
            .poll_write_header_once(cx, this.inner.as_mut()))?;

        this.inner.poll_write(cx, buf)
    }
    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), io::Error>> {
        self.project().inner.poll_flush(cx)
    }
    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), io::Error>> {
        self.project().inner.poll_shutdown(cx)
    }
}

#[cfg(feature = "hyper")]
mod hyper {
    use ::hyper::rt::{
        Read as HyperRead,
        Write as HyperWrite,
    };

    use super::*;

    impl<S> HyperWrite for Stream<S>
    where
        S: AsyncWrite,
    {
        #[instrument(level = "trace", skip(self), fields(write_state = ?self.write_state))]
        fn poll_write(
            self: Pin<&mut Self>,
            cx: &mut Context<'_>,
            buf: &[u8],
        ) -> Poll<Result<usize, io::Error>> {
            <Self as AsyncWrite>::poll_write(self, cx, buf)
        }
        fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), io::Error>> {
            <Self as AsyncWrite>::poll_flush(self, cx)
        }
        fn poll_shutdown(
            self: Pin<&mut Self>,
            cx: &mut Context<'_>,
        ) -> Poll<Result<(), io::Error>> {
            <Self as AsyncWrite>::poll_shutdown(self, cx)
        }
    }

    impl<S> HyperRead for Stream<S>
    where
        S: AsyncRead,
    {
        fn poll_read(
            self: Pin<&mut Self>,
            cx: &mut Context<'_>,
            mut buf: ::hyper::rt::ReadBufCursor<'_>,
        ) -> Poll<Result<(), std::io::Error>> {
            let mut tokio_buf = tokio::io::ReadBuf::uninit(unsafe { buf.as_mut() });
            let res = ready!(<Self as AsyncRead>::poll_read(self, cx, &mut tokio_buf));
            let filled = tokio_buf.filled().len();
            unsafe { buf.advance(filled) };
            Poll::Ready(res)
        }
    }
}

#[cfg(test)]
mod test {
    use std::{
        cmp,
        io,
        pin::Pin,
        task::{
            ready,
            Context,
            Poll,
        },
        time::Duration,
    };

    use bytes::{
        BufMut,
        BytesMut,
    };
    use proxy_protocol::{
        version2::{
            self,
            ProxyCommand,
        },
        ProxyHeader,
    };
    use tokio::io::{
        AsyncRead,
        AsyncReadExt,
        AsyncWriteExt,
        ReadBuf,
    };

    use super::Stream;

    #[pin_project::pin_project]
    struct ShortReader<S> {
        #[pin]
        inner: S,
        min: usize,
        max: usize,
    }

    impl<S> AsyncRead for ShortReader<S>
    where
        S: AsyncRead,
    {
        fn poll_read(
            self: Pin<&mut Self>,
            cx: &mut Context<'_>,
            buf: &mut ReadBuf<'_>,
        ) -> Poll<io::Result<()>> {
            let mut this = self.project();
            let max_bytes =
                *this.min + cmp::max(1, rand::random::<usize>() % (*this.max - *this.min));
            let mut tmp = vec![0; max_bytes];
            let mut tmp_buf = ReadBuf::new(&mut tmp);
            let res = ready!(this.inner.as_mut().poll_read(cx, &mut tmp_buf));

            buf.put_slice(tmp_buf.filled());

            res?;

            Poll::Ready(Ok(()))
        }
    }

    impl<S> ShortReader<S> {
        fn new(inner: S, min: usize, max: usize) -> Self {
            ShortReader { inner, min, max }
        }
    }

    const INPUT: &str = "PROXY TCP4 192.168.0.1 192.168.0.11 56324 443\r\n";
    const PARTIAL_INPUT: &str = "PROXY TCP4 192.168.0.1";
    const FINAL_INPUT: &str = " 192.168.0.11 56324 443\r\n";

    // Smoke test to ensure that the proxy protocol parser works as expected.
    // Not actually testing our code.
    #[test]
    fn test_proxy_protocol() {
        let mut buf = BytesMut::from(INPUT);

        assert!(proxy_protocol::parse(&mut buf).is_ok());

        buf = BytesMut::from(PARTIAL_INPUT);

        assert!(proxy_protocol::parse(&mut &*buf).is_err());

        buf.put_slice(FINAL_INPUT.as_bytes());

        assert!(proxy_protocol::parse(&mut &*buf).is_ok());
    }

    #[tokio::test]
    #[tracing_test::traced_test]
    async fn test_header_stream_v2() {
        let (left, mut right) = tokio::io::duplex(1024);

        let header = ProxyHeader::Version2 {
            command: ProxyCommand::Proxy,
            transport_protocol: version2::ProxyTransportProtocol::Stream,
            addresses: version2::ProxyAddresses::Ipv4 {
                source: "127.0.0.1:1".parse().unwrap(),
                destination: "127.0.0.2:2".parse().unwrap(),
            },
        };

        let input = proxy_protocol::encode(header).unwrap();

        let mut proxy_stream = Stream::incoming(ShortReader::new(left, 2, 5));

        // Chunk our writes to ensure that our reader is resilient across split inputs.
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(50)).await;

            right.write_all(&input).await.expect("write header");

            right
                .write_all(b"Hello, world!")
                .await
                .expect("write hello");

            right.shutdown().await.expect("shutdown");
        });

        let hdr = proxy_stream
            .proxy_header()
            .await
            .expect("read header")
            .expect("decode header")
            .expect("header exists");

        assert!(matches!(hdr, ProxyHeader::Version2 { .. }));

        let mut buf = String::new();

        proxy_stream
            .read_to_string(&mut buf)
            .await
            .expect("read rest");

        assert_eq!(buf, "Hello, world!");

        // Get the header again - should be the same.
        let hdr = proxy_stream
            .proxy_header()
            .await
            .expect("read header")
            .expect("decode header")
            .expect("header exists");

        assert!(matches!(hdr, ProxyHeader::Version2 { .. }));
    }

    #[tokio::test]
    #[tracing_test::traced_test]
    async fn test_header_stream() {
        let (left, mut right) = tokio::io::duplex(1024);

        let mut proxy_stream = Stream::incoming(ShortReader::new(left, 2, 5));

        // Chunk our writes to ensure that our reader is resilient across split inputs.
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(50)).await;

            right
                .write_all(INPUT.as_bytes())
                .await
                .expect("write header");

            right
                .write_all(b"Hello, world!")
                .await
                .expect("write hello");

            right.shutdown().await.expect("shutdown");
        });

        let hdr = proxy_stream
            .proxy_header()
            .await
            .expect("read header")
            .expect("decode header")
            .expect("header exists");

        assert!(matches!(hdr, ProxyHeader::Version1 { .. }));

        let mut buf = String::new();

        proxy_stream
            .read_to_string(&mut buf)
            .await
            .expect("read rest");

        assert_eq!(buf, "Hello, world!");

        // Get the header again - should be the same.
        let hdr = proxy_stream
            .proxy_header()
            .await
            .expect("read header")
            .expect("decode header")
            .expect("header exists");

        assert!(matches!(hdr, ProxyHeader::Version1 { .. }));
    }

    #[tokio::test]
    #[tracing_test::traced_test]
    async fn test_noheader() {
        let (left, mut right) = tokio::io::duplex(1024);

        let mut proxy_stream = Stream::incoming(left);

        right
            .write_all(b"Hello, world!")
            .await
            .expect("write stream");

        right.shutdown().await.expect("shutdown");
        drop(right);

        assert!(proxy_stream
            .proxy_header()
            .await
            .unwrap()
            .unwrap()
            .is_none());

        let mut buf = String::new();

        proxy_stream
            .read_to_string(&mut buf)
            .await
            .expect("read stream");

        assert_eq!(buf, "Hello, world!");
    }
}
