#[cfg(target_os = "windows")]
use std::time::Duration;
#[cfg(feature = "hyper")]
use std::{
    convert::Infallible,
    fmt,
};
use std::{
    io,
    net::SocketAddr,
    path::Path,
};

use async_trait::async_trait;
use futures::stream::TryStreamExt;
#[cfg(feature = "hyper")]
use hyper::{
    server::conn::Http,
    service::service_fn,
    Body,
    Response,
    StatusCode,
};
#[cfg(target_os = "windows")]
use tokio::net::windows::named_pipe::ClientOptions;
#[cfg(not(target_os = "windows"))]
use tokio::net::UnixStream;
#[cfg(target_os = "windows")]
use tokio::time;
use tokio::{
    io::{
        copy_bidirectional,
        AsyncRead,
        AsyncWrite,
    },
    net::{
        TcpStream,
        ToSocketAddrs,
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
#[cfg(target_os = "windows")]
use windows_sys::Win32::Foundation::ERROR_PIPE_BUSY;

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
    #[cfg(feature = "hyper")]
    #[instrument(level = "debug", skip_all, fields(local_addrs))]
    async fn forward_http(&mut self, addr: impl ToSocketAddrs + Send) -> Result<(), io::Error> {
        forward_conns(self, addr, |e, c| drop(serve_gateway_error(e, c))).await
    }

    /// Forward incoming tunnel connections to the provided file socket path.
    /// On Linux/Darwin addr can be a unix domain socket path, e.g. "/tmp/ngrok.sock".
    /// On Windows addr can be a named pipe, e.g. "\\.\pipe\ngrok_pipe".
    #[instrument(level = "debug", skip_all, fields(path))]
    async fn forward_pipe(&mut self, addr: impl AsRef<Path> + Send) -> Result<(), io::Error> {
        forward_pipe_conns(self, addr, |_, _| {}).await
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

async fn forward_pipe_conns<T, F>(
    this: &mut T,
    addr: impl AsRef<Path>,
    mut on_err: F,
) -> Result<(), io::Error>
where
    T: Tunnel + ?Sized,
    F: FnMut(io::Error, Conn),
{
    let span = Span::current();
    let path = addr.as_ref();
    span.record("path", field::debug(&path));
    loop {
        trace!("waiting for new tunnel connection");
        if !handle_one_pipe(this, path, &mut on_err).await? {
            debug!("listener closed, exiting");
            break;
        }
    }
    Ok(())
}

fn join_streams(
    mut left: impl AsyncRead + AsyncWrite + Unpin + Send + 'static,
    mut right: impl AsyncRead + AsyncWrite + Unpin + Send + 'static,
) -> JoinHandle<()> {
    tokio::spawn(
        async move {
            match copy_bidirectional(&mut left, &mut right).await {
                Ok((l_bytes, r_bytes)) => debug!("joined streams closed, bytes from tunnel: {l_bytes}, bytes from local: {r_bytes}"),
                Err(e) => debug!("joined streams error: {e}"),
            };
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
async fn handle_one_pipe<T, F>(this: &mut T, addr: &Path, on_error: F) -> Result<bool, io::Error>
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

    #[cfg(not(target_os = "windows"))]
    {
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
    }
    #[cfg(target_os = "windows")]
    {
        // loop behavior copied from docs
        // https://docs.rs/tokio/latest/tokio/net/windows/named_pipe/struct.NamedPipeClient.html
        let local_conn = loop {
            match ClientOptions::new().open(addr) {
                Ok(client) => break client,
                Err(error) if error.raw_os_error() == Some(ERROR_PIPE_BUSY as i32) => (),
                Err(error) => {
                    warn!(%error, "error establishing local named pipe connection");
                    on_error(error, tunnel_conn);
                    return Ok(true);
                }
            }

            time::sleep(Duration::from_millis(50)).await;
        };
        span.record("local_addr", field::debug(addr));
        debug!("established local connection, joining streams");
        join_streams(tunnel_conn, local_conn);
    }

    Ok(true)
}

#[cfg(feature = "hyper")]
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
