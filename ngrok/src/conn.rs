use std::{
    net::SocketAddr,
    pin::Pin,
    task::{
        Context,
        Poll,
    },
};

// Support for axum's connection info trait.
#[cfg(feature = "axum")]
use axum::extract::connect_info::Connected;
#[cfg(feature = "hyper")]
use hyper::rt::{
    Read as HyperRead,
    Write as HyperWrite,
};
use muxado::typed::TypedStream;
use tokio::io::{
    AsyncRead,
    AsyncWrite,
};

use crate::{
    config::ProxyProto,
    internals::proto::{
        EdgeType,
        ProxyHeader,
    },
};
/// A connection from an ngrok tunnel.
///
/// This implements [AsyncRead]/[AsyncWrite], as well as providing access to the
/// address from which the connection to the ngrok edge originated.
pub(crate) struct ConnInner {
    pub(crate) info: Info,
    pub(crate) stream: TypedStream,
}

#[derive(Clone)]
pub(crate) struct Info {
    pub(crate) header: ProxyHeader,
    pub(crate) remote_addr: SocketAddr,
    pub(crate) proxy_proto: ProxyProto,
    pub(crate) app_protocol: Option<String>,
    pub(crate) verify_upstream_tls: bool,
}

impl ConnInfo for Info {
    fn remote_addr(&self) -> SocketAddr {
        self.remote_addr
    }
}

impl EdgeConnInfo for Info {
    fn edge_type(&self) -> EdgeType {
        self.header.edge_type
    }
    fn passthrough_tls(&self) -> bool {
        self.header.passthrough_tls
    }
}

impl EndpointConnInfo for Info {
    fn proto(&self) -> &str {
        self.header.proto.as_str()
    }
}

// This codgen indirect is required to make the hyper io trait bounds
// dependent on the hyper feature. You can't put a #[cfg] on a single bound, so
// we're putting the whole trait def in a macro. Gross, but gets the job done.
macro_rules! conn_trait {
    ($($hyper_bound:tt)*) => {
		/// An incoming connection over an ngrok tunnel.
		/// Effectively a trait alias for async read+write, plus connection info.
		pub trait Conn: ConnInfo + AsyncRead + AsyncWrite $($hyper_bound)* + Unpin + Send + 'static {}
	}
}

#[cfg(not(feature = "hyper"))]
conn_trait!();

#[cfg(feature = "hyper")]
conn_trait! {
    + hyper::rt::Read + hyper::rt::Write
}

/// Information common to all ngrok connections.
pub trait ConnInfo {
    /// Returns the client address that initiated the connection to the ngrok
    /// edge.
    fn remote_addr(&self) -> SocketAddr;
}

/// Information about connections via ngrok edges.
pub trait EdgeConnInfo {
    /// Returns the edge type for this connection.
    fn edge_type(&self) -> EdgeType;
    /// Returns whether the connection includes the tls handshake and encrypted
    /// stream.
    fn passthrough_tls(&self) -> bool;
}

/// Information about connections via ngrok endpoints.
pub trait EndpointConnInfo {
    /// Returns the endpoint protocol.
    fn proto(&self) -> &str;
}

macro_rules! make_conn_type {
	(info EdgeConnInfo, $wrapper:tt) => {
		impl EdgeConnInfo for $wrapper {
			fn edge_type(&self) -> EdgeType {
				self.inner.info.edge_type()
			}
			fn passthrough_tls(&self) -> bool {
				self.inner.info.passthrough_tls()
			}
		}
	};
	(info EndpointConnInfo, $wrapper:tt) => {
		impl EndpointConnInfo for $wrapper {
			fn proto(&self) -> &str {
				self.inner.info.proto()
			}
		}
	};
    ($(#[$outer:meta])* $wrapper:ident, $($m:tt),*) => {
        $(#[$outer])*
        pub struct $wrapper {
            pub(crate) inner: ConnInner,
        }

        impl Conn for $wrapper {}

        impl ConnInfo for $wrapper {
			fn remote_addr(&self) -> SocketAddr {
				self.inner.info.remote_addr()
			}
        }

		impl AsyncRead for $wrapper {
			fn poll_read(
				mut self: Pin<&mut Self>,
				cx: &mut Context<'_>,
				buf: &mut tokio::io::ReadBuf<'_>,
			) -> Poll<std::io::Result<()>> {
				Pin::new(&mut *self.inner.stream).poll_read(cx, buf)
			}
		}

		#[cfg(feature = "hyper")]
		impl HyperRead for $wrapper {
			fn poll_read(
				mut self: Pin<&mut Self>,
				cx: &mut Context<'_>,
				mut buf: hyper::rt::ReadBufCursor<'_>,
			) -> Poll<std::io::Result<()>> {
				let mut tokio_buf = tokio::io::ReadBuf::uninit(unsafe{ buf.as_mut() });
				let res = std::task::ready!(Pin::new(&mut *self.inner.stream).poll_read(cx, &mut tokio_buf));
				let filled = tokio_buf.filled().len();
				unsafe { buf.advance(filled) };
				Poll::Ready(res)
			}
		}

		#[cfg(feature = "hyper")]
		impl HyperWrite for $wrapper {
			fn poll_write(
				mut self: Pin<&mut Self>,
				cx: &mut Context<'_>,
				buf: &[u8],
			) -> Poll<Result<usize, std::io::Error>> {
				Pin::new(&mut *self.inner.stream).poll_write(cx, buf)
			}
			fn poll_flush(
				mut self: Pin<&mut Self>,
				cx: &mut Context<'_>,
			) -> Poll<Result<(), std::io::Error>> {
				Pin::new(&mut *self.inner.stream).poll_flush(cx)
			}
			fn poll_shutdown(
				mut self: Pin<&mut Self>,
				cx: &mut Context<'_>,
			) -> Poll<Result<(), std::io::Error>> {
				Pin::new(&mut *self.inner.stream).poll_shutdown(cx)
			}
		}

		impl AsyncWrite for $wrapper {
			fn poll_write(
				mut self: Pin<&mut Self>,
				cx: &mut Context<'_>,
				buf: &[u8],
			) -> Poll<Result<usize, std::io::Error>> {
				Pin::new(&mut *self.inner.stream).poll_write(cx, buf)
			}
			fn poll_flush(
				mut self: Pin<&mut Self>,
				cx: &mut Context<'_>,
			) -> Poll<Result<(), std::io::Error>> {
				Pin::new(&mut *self.inner.stream).poll_flush(cx)
			}
			fn poll_shutdown(
				mut self: Pin<&mut Self>,
				cx: &mut Context<'_>,
			) -> Poll<Result<(), std::io::Error>> {
				Pin::new(&mut *self.inner.stream).poll_shutdown(cx)
			}
		}

		#[cfg_attr(docsrs, doc(cfg(feature = "axum")))]
		#[cfg(feature = "axum")]
		impl Connected<&$wrapper> for SocketAddr {
			fn connect_info(target: &$wrapper) -> Self {
				target.inner.info.remote_addr()
			}
		}

		$(
			make_conn_type!(info $m, $wrapper);
		)*
    };
}

make_conn_type! {
    /// A connection via an ngrok Edge.
    EdgeConn, EdgeConnInfo
}

make_conn_type! {
    /// A connection via an ngrok Endpoint.
    EndpointConn, EndpointConnInfo
}
