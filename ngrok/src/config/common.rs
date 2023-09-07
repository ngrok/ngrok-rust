use std::{
    collections::HashMap,
    env,
    process,
};
#[cfg(feature = "hyper")]
use std::{
    error::Error as StdError,
    future::Future,
};

use async_trait::async_trait;
#[cfg(feature = "hyper")]
use hyper::{
    body::HttpBody,
    service::Service,
    Body,
    Request,
    Response,
};
use once_cell::sync::OnceCell;
use url::Url;

pub use crate::internals::proto::ProxyProto;
use crate::{
    forwarder::Forwarder,
    internals::proto::{
        BindExtra,
        BindOpts,
        IpRestriction,
        MutualTls,
    },
    session::RpcError,
    Session,
    Tunnel,
};

pub(crate) fn default_forwards_to() -> &'static str {
    static FORWARDS_TO: OnceCell<String> = OnceCell::new();

    FORWARDS_TO
        .get_or_init(|| {
            let hostname = hostname::get()
                .unwrap_or("<unknown>".into())
                .to_string_lossy()
                .into_owned();
            let exe = env::current_exe()
                .unwrap_or("<unknown>".into())
                .to_string_lossy()
                .into_owned();
            let pid = process::id();
            format!("app://{hostname}/{exe}?pid={pid}")
        })
        .as_str()
}

/// Trait representing things that can be built into an ngrok tunnel.
#[async_trait]
pub trait TunnelBuilder: From<Session> {
    /// The ngrok tunnel type that this builder produces.
    type Tunnel: Tunnel;

    /// Begin listening for new connections on this tunnel.
    async fn listen(&self) -> Result<Self::Tunnel, RpcError>;
}

/// Trait representing things that can be built into an ngrok tunnel and then
/// forwarded to a provided URL.
#[async_trait]
pub trait ForwarderBuilder: TunnelBuilder {
    /// Start listening for new connections on this tunnel and forward all
    /// connections to the provided URL.
    async fn listen_and_forward(&self, to_url: Url) -> Result<Forwarder<Self::Tunnel>, RpcError>;

    #[cfg(feature = "hyper")]
    /// Start listening for new connections on this tunnel and serve them all
    /// via the provided service.
    async fn listen_and_serve<S, R, B, D, BE, E, SE, F, FE, SF>(
        &self,
        service: S,
    ) -> Result<Forwarder<Self::Tunnel>, RpcError>
    where
        for<'a> S: Service<&'a crate::Conn, Response = R, Error = E, Future = SF> + Send + 'static,
        R: Service<Request<Body>, Response = Response<B>, Error = SE, Future = F> + Send + 'static,
        B: HttpBody<Data = D, Error = BE> + Send + 'static,
        D: Send + 'static,
        BE: StdError + Send + Sync + 'static,
        E: StdError + Send + Sync + 'static,
        FE: StdError + Send + Sync + 'static,
        SE: StdError + Send + Sync + 'static,
        F: Future<Output = Result<Response<B>, FE>> + Send + 'static,
        SF: Future<Output = Result<R, E>> + Send + 'static;
}

macro_rules! impl_builder {
    ($(#[$m:meta])* $name:ident, $opts:ty, $tun:ident, $get_proto:expr) => {
        $(#[$m])*
        #[derive(Clone)]
        pub struct $name {
            options: $opts,
            // Note: This is only optional for testing purposes.
            session: Option<Session>,
        }

        mod __builder_impl {
            use $crate::forwarder::Forwarder;
            use $crate::config::common::ForwarderBuilder;
            use $crate::config::common::TunnelBuilder;
            use $crate::session::RpcError;
            use async_trait::async_trait;
            use url::Url;
            #[cfg(feature = "hyper")]
            use hyper::{
                body::HttpBody,
                Response,
                Request,
                service::Service,
                Body,
            };
            #[cfg(feature = "hyper")]
            use std::{
                error::Error as StdError,
                future::Future,
            };

            use super::*;

            impl From<Session> for $name {
                fn from(session: Session) -> Self {
                    $name {
                        options: Default::default(),
                        session: session.into(),
                    }
                }
            }

            #[async_trait]
            impl TunnelBuilder for $name {
                type Tunnel = $tun;

                async fn listen(&self) -> Result<$tun, RpcError> {
                    Ok($tun {
                        inner: self
                            .session
                            .as_ref()
                            .unwrap()
                            .start_tunnel(&self.options)
                            .await?,
                    })
                }
            }

            #[async_trait]
            impl ForwarderBuilder for $name {
                async fn listen_and_forward(&self, to_url: Url) -> Result<Forwarder<$tun>, RpcError> {
                    let mut cfg = self.clone();
                    let url_str: &str = to_url.as_ref();
                    cfg.forwards_to(url_str);
                    let tunnel = cfg.listen().await?;
                    let info = tunnel.make_info();
                    $crate::forwarder::forward(tunnel, info, to_url)
                }

                #[cfg(feature = "hyper")]
                async fn listen_and_serve<S, R, B, D, BE, E, SE, F, FE, SF>(
                    &self,
                    service: S,
                ) -> Result<Forwarder<Self::Tunnel>, RpcError>
                where
                    for<'a> S: Service<&'a crate::Conn, Response = R, Error = E, Future = SF> + Send + 'static,
                    R: Service<Request<Body>, Response = Response<B>, Error = SE, Future = F>
                        + Send
                        + 'static,
                    B: HttpBody<Data = D, Error = BE> + Send + 'static,
                    D: Send + 'static,
                    BE: StdError + Send + Sync + 'static,
                    E: StdError + Send + Sync + 'static,
                    FE: StdError + Send + Sync + 'static,
                    SE: StdError + Send + Sync + 'static,
                    F: Future<Output = Result<Response<B>, FE>> + Send + 'static,
                    SF: Future<Output = Result<R, E>> + Send + 'static
                {
                    let mut cfg = self.clone();
                    cfg.forwards_to(default_forwards_to());
                    let tunnel = cfg.listen().await?;
                    let info = tunnel.make_info();
                    $crate::forwarder::serve_http(tunnel, info, service)
                }
            }
        }
    };
}

/// Tunnel configuration trait, implemented by our top-level config objects.
pub(crate) trait TunnelConfig {
    /// The "forwards to" metadata.
    ///
    /// Only for display/informational purposes.
    fn forwards_to(&self) -> String;
    /// Internal-only, extra data sent when binding a tunnel.
    fn extra(&self) -> BindExtra;
    /// The protocol for this tunnel.
    fn proto(&self) -> String;
    /// The middleware and other configuration options for this tunnel.
    fn opts(&self) -> Option<BindOpts>;
    /// The labels for this tunnel.
    fn labels(&self) -> HashMap<String, String>;
}

// delegate references
impl<'a, T> TunnelConfig for &'a T
where
    T: TunnelConfig,
{
    fn forwards_to(&self) -> String {
        (**self).forwards_to()
    }
    fn extra(&self) -> BindExtra {
        (**self).extra()
    }
    fn proto(&self) -> String {
        (**self).proto()
    }
    fn opts(&self) -> Option<BindOpts> {
        (**self).opts()
    }
    fn labels(&self) -> HashMap<String, String> {
        (**self).labels()
    }
}

/// Restrictions placed on the origin of incoming connections to the edge.
#[derive(Clone, Default)]
pub(crate) struct CidrRestrictions {
    /// Rejects connections that do not match the given CIDRs
    pub(crate) allowed: Vec<String>,
    /// Rejects connections that match the given CIDRs and allows all other CIDRs.
    pub(crate) denied: Vec<String>,
}

impl CidrRestrictions {
    pub(crate) fn allow(&mut self, cidr: impl Into<String>) {
        self.allowed.push(cidr.into());
    }
    pub(crate) fn deny(&mut self, cidr: impl Into<String>) {
        self.denied.push(cidr.into());
    }
}

// Common
#[derive(Default, Clone)]
pub(crate) struct CommonOpts {
    // Restrictions placed on the origin of incoming connections to the edge.
    pub(crate) cidr_restrictions: CidrRestrictions,
    // The version of PROXY protocol to use with this tunnel, zero if not
    // using.
    pub(crate) proxy_proto: ProxyProto,
    // Tunnel-specific opaque metadata. Viewable via the API.
    pub(crate) metadata: Option<String>,
    // Tunnel backend metadata. Viewable via the dashboard and API, but has no
    // bearing on tunnel behavior.
    pub(crate) forwards_to: Option<String>,
}

impl CommonOpts {
    // Get the proto version of cidr restrictions
    pub(crate) fn ip_restriction(&self) -> Option<IpRestriction> {
        (!self.cidr_restrictions.allowed.is_empty() || !self.cidr_restrictions.denied.is_empty())
            .then_some(self.cidr_restrictions.clone().into())
    }
}

// transform into the wire protocol format
impl From<CidrRestrictions> for IpRestriction {
    fn from(cr: CidrRestrictions) -> Self {
        IpRestriction {
            allow_cidrs: cr.allowed,
            deny_cidrs: cr.denied,
        }
    }
}

impl From<&[bytes::Bytes]> for MutualTls {
    fn from(b: &[bytes::Bytes]) -> Self {
        let mut aggregated = Vec::new();
        b.iter().for_each(|c| aggregated.extend(c));
        MutualTls {
            mutual_tls_ca: aggregated,
        }
    }
}
