pub(crate) const FORWARDS_TO: &str = "rust";

// Tunnel configuration trait, implemented by our top-level config objects.
// "Sealed," i.e. not implementable outside of the crate.
pub trait TunnelConfig: private::TunnelConfigPrivate {
	//type Tunnel: AsyncRead + AsyncWrite;
}

// Non-exported private tunnel config type that "seals" the exported one
// This is where we'll produce the config struct that ultimately gets passed
// to the tunnel bind RPCs.
// private: https://rust-lang.github.io/api-guidelines/future-proofing.html
// avoids "A private trait was used on a public type parameter bound"
//   https://doc.rust-lang.org/error_codes/E0445.html
pub(crate) mod private {
    use std::collections::HashMap;

    use crate::{internals::proto::{BindExtra, BindOpts}};

    // This is the internal-only interface that all config.Tunnel implementations
    // *also* implement. This lets us pull the necessary bits out of it without
    // polluting the public interface with internal details.
    pub trait TunnelConfigPrivate {
        fn forwards_to(&self) -> String;
        fn extra(&self) -> BindExtra;
        fn proto(&self) -> String;
        fn opts(&self) -> Option<BindOpts>;
        fn labels(&self) -> HashMap<String,String>;
    }    
}

// Our top-level tunnel configration structures.
// All implement Default.
// All use the builder pattern to set options.
// All implement TunnelConfig, along with references to them so that it's all chainable.

// Common
pub(crate) struct CommonOpts {
	// Restrictions placed on the origin of incoming connections to the edge.
	// cidr_restrictions *cidrRestrictions, // todo
	// The version of PROXY protocol to use with this tunnel, zero if not
	// using.
	// proxy_proto: ProxyProtoVersion, // todo
	// Tunnel-specific opaque metadata. Viewable via the API.
	pub(crate) metadata: Option<String>,
	// Tunnel backend metadata. Viewable via the dashboard and API, but has no
	// bearing on tunnel behavior.
	pub(crate) forwards_to: Option<String>,
}

impl Default for CommonOpts {
    fn default() -> Self {
        CommonOpts {
            metadata: None,
            forwards_to: None,
        }
    }
}