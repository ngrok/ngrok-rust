use std::collections::HashMap;

use prost::bytes;

pub use crate::internals::proto::ProxyProto;
use crate::{
    internals::proto::{
        BindExtra,
        BindOpts,
    },
    mw::middleware_configuration::{
        IpRestriction,
        MutualTls,
    },
};

pub(crate) const FORWARDS_TO: &str = "rust";

// Tunnel configuration trait, implemented by our top-level config objects.
// "Sealed," i.e. not implementable outside of the crate.
pub trait TunnelConfig: private::Sealed {
    fn forwards_to(&self) -> String;
    fn extra(&self) -> BindExtra;
    fn proto(&self) -> String;
    fn opts(&self) -> Option<BindOpts>;
    fn labels(&self) -> HashMap<String, String>;
}

// delegate references
impl<'a, T> TunnelConfig for &'a mut T
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

// Non-exported private tunnel config type that "seals" the exported one
// This is where we'll produce the config struct that ultimately gets passed
// to the tunnel bind RPCs.
// private: https://rust-lang.github.io/api-guidelines/future-proofing.html
// avoids "A private trait was used on a public type parameter bound"
//   https://doc.rust-lang.org/error_codes/E0445.html
// or "can't leak crate-private trait"
//   https://users.rust-lang.org/t/pub-trait-in-private-module-no-cant-leak-private-trait-error/46052
pub(crate) mod private {
    pub trait Sealed {}

    // delegate references
    impl<'a, T> Sealed for &'a T where T: Sealed {}

    // delegate mutable references
    impl<'a, T> Sealed for &'a mut T where T: Sealed {}
}

/// Restrictions placed on the origin of incoming connections to the edge.
#[derive(Clone, Default)]
pub struct CidrRestrictions {
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
#[derive(Default)]
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
