use std::{
    convert::Infallible,
    fmt,
    io::{self,},
    net::SocketAddr,
    path::Path,
};

use async_trait::async_trait;
use futures::stream::TryStreamExt;
use hyper::{
    server::conn::Http,
    service::service_fn,
    Body,
    Response,
    StatusCode,
};
use tokio::{
    io::{
        AsyncRead,
        AsyncReadExt,
        AsyncWrite,
        AsyncWriteExt,
    },
    net::{
        TcpStream,
        ToSocketAddrs,
        UnixStream,
    },
    task::JoinHandle,
};
use tracing::{
    debug,
    field,
    instrument,
    trace,
    warn,
    Instrument,
    Span,
};

use crate::{
    prelude::*,
    Conn,
};

impl<T> TunnelExt for T where T: Tunnel {}

/// Extension methods auto-implemented for all tunnel types
#[async_trait]
pub trait TunnelExt: Tunnel {
    /// Forward incoming tunnel connections to the provided TCP address.
    #[instrument(level = "debug", skip_all, fields(local_addrs))]
    async fn forward_tcp(&mut self, addr: impl ToSocketAddrs + Send) -> Result<(), io::Error> {
        forward_conns(self, addr, |_, _| {}).await
    }

    /// Forward incoming tunnel connections to the provided TCP address.
    ///
    /// Provides slightly nicer errors when the backend is unavailable.
    #[instrument(level = "debug", skip_all, fields(local_addrs))]
    async fn forward_http(&mut self, addr: impl ToSocketAddrs + Send) -> Result<(), io::Error> {
        forward_conns(self, addr, |e, c| drop(serve_gateway_error(e, c))).await
    }

    /// Forward incoming tunnel connections to the provided Unix socket path.
    #[instrument(level = "debug", skip_all, fields(local_addrs))]
    async fn forward_unix(&mut self, addr: String) -> Result<(), io::Error> {
        forward_unix_conns(self, addr, |_, _| {}).await
    }
}

async fn forward_conns<T, A, F>(this: &mut T, addr: A, mut on_err: F) -> Result<(), io::Error>
where
    T: Tunnel + ?Sized,
    A: ToSocketAddrs,
    F: FnMut(io::Error, Conn),
{
    let span = Span::current();
    let addrs = tokio::net::lookup_host(addr).await?.collect::<Vec<_>>();
    span.record("local_addrs", field::debug(&addrs));
    trace!("looked up local addrs");
    loop {
        trace!("waiting for new tunnel connection");
        if !handle_one(this, addrs.as_slice(), &mut on_err).await? {
            debug!("listener closed, exiting");
            break;
        }
    }
    Ok(())
}

async fn forward_unix_conns<T, F>(
    this: &mut T,
    addr: String,
    mut on_err: F,
) -> Result<(), io::Error>
where
    T: Tunnel + ?Sized,
    F: FnMut(io::Error, Conn),
{
    let span = Span::current();
    let path = Path::new(addr.as_str());
    span.record("path", field::debug(&path));
    trace!("looked up local path");
    loop {
        trace!("waiting for new tunnel connection");
        if !handle_one_unix(this, path, &mut on_err).await? {
            debug!("listener closed, exiting");
            break;
        }
    }
    Ok(())
}

async fn forward_bytes(
    mut r: impl AsyncRead + Unpin,
    mut w: impl AsyncWrite + Unpin,
) -> Result<(), io::Error> {
    let mut buf = vec![0u8; 1024];
    loop {
        let n = r.read(&mut buf).await?;
        if n == 0 {
            break;
        }

        w.write_all(&buf[0..n]).await?;
    }
    Ok(())
}

fn join_streams(
    left: impl AsyncRead + AsyncWrite + Unpin + Send + 'static,
    right: impl AsyncRead + AsyncWrite + Unpin + Send + 'static,
) -> JoinHandle<()> {
    let (l_rx, l_tx) = tokio::io::split(left);
    let (r_rx, r_tx) = tokio::io::split(right);

    let joined = futures::future::join(forward_bytes(r_rx, l_tx), forward_bytes(l_rx, r_tx));
    tokio::spawn(
        async move {
            let (to_tunnel, to_local) = joined.await;
            debug!(?to_tunnel, ?to_local, "connection closed");
        }
        .in_current_span(),
    )
}

#[instrument(level = "debug", skip_all, fields(remote_addr, local_addr))]
async fn handle_one<T, F>(
    this: &mut T,
    addrs: &[SocketAddr],
    on_error: F,
) -> Result<bool, io::Error>
where
    T: Tunnel + ?Sized,
    F: FnOnce(io::Error, Conn),
{
    let span = Span::current();
    let tunnel_conn = if let Some(conn) = this
        .try_next()
        .await
        .map_err(|err| io::Error::new(io::ErrorKind::NotConnected, err))?
    {
        conn
    } else {
        return Ok(false);
    };

    span.record("remote_addr", field::debug(tunnel_conn.remote_addr()));

    trace!("accepted tunnel connection");

    let local_conn = match TcpStream::connect(addrs).await {
        Ok(conn) => conn,
        Err(error) => {
            warn!(%error, "error establishing local connection");

            on_error(error, tunnel_conn);

            return Ok(true);
        }
    };
    span.record("local_addr", field::debug(local_conn.peer_addr().unwrap()));

    debug!("established local connection, joining streams");

    join_streams(tunnel_conn, local_conn);
    Ok(true)
}

#[instrument(level = "debug", skip_all, fields(remote_addr, local_addr))]
async fn handle_one_unix<T, F>(this: &mut T, addr: &Path, on_error: F) -> Result<bool, io::Error>
where
    T: Tunnel + ?Sized,
    F: FnOnce(io::Error, Conn),
{
    let span = Span::current();
    let tunnel_conn = if let Some(conn) = this
        .try_next()
        .await
        .map_err(|err| io::Error::new(io::ErrorKind::NotConnected, err))?
    {
        conn
    } else {
        return Ok(false);
    };

    span.record("remote_addr", field::debug(tunnel_conn.remote_addr()));

    trace!("accepted tunnel connection");

    let local_conn = match UnixStream::connect(addr).await {
        Ok(conn) => conn,
        Err(error) => {
            warn!(%error, "error establishing local unix connection");

            on_error(error, tunnel_conn);

            return Ok(true);
        }
    };
    span.record("local_addr", field::debug(local_conn.peer_addr().unwrap()));

    debug!("established local connection, joining streams");

    join_streams(tunnel_conn, local_conn);
    Ok(true)
}

#[allow(dead_code)]
fn serve_gateway_error(
    err: impl fmt::Display + Send + 'static,
    stream: impl AsyncRead + AsyncWrite + Unpin + Send + 'static,
) -> JoinHandle<()> {
    tokio::spawn(
        async move {
            let res = Http::new()
                .http1_only(true)
                .http1_keep_alive(false)
                .serve_connection(
                    stream,
                    service_fn(move |_req| {
                        debug!("serving bad gateway error");
                        let mut resp =
                            Response::new(Body::from(format!("failed to dial backend: {err}")));
                        *resp.status_mut() = StatusCode::BAD_GATEWAY;
                        futures::future::ok::<_, Infallible>(resp)
                    }),
                )
                .await;
            debug!(?res, "connection closed");
        }
        .in_current_span(),
    )
}
