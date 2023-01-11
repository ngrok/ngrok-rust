use crate::internals::proto::{
    Oidc,
    SecretString,
};

/// Oidc Options configuration
#[derive(Clone, Default)]
pub struct OidcOptions {
    issuer_url: String,
    client_id: String,
    client_secret: SecretString,
    allow_emails: Vec<String>,
    allow_domains: Vec<String>,
    scopes: Vec<String>,
}

impl OidcOptions {
    /// Create a new [OidcOptions] with the given issuer and client information.
    pub fn new(
        issuer_url: impl Into<String>,
        client_id: impl Into<String>,
        client_secret: impl Into<String>,
    ) -> Self {
        OidcOptions {
            issuer_url: issuer_url.into(),
            client_id: client_id.into(),
            client_secret: client_secret.into().into(),
            ..Default::default()
        }
    }

    /// Allow the oidc user with the given email to access the tunnel.
    pub fn allow_email(mut self, email: impl Into<String>) -> Self {
        self.allow_emails.push(email.into());
        self
    }
    /// Allow the oidc user with the given email domain to access the tunnel.
    pub fn allow_domain(mut self, domain: impl Into<String>) -> Self {
        self.allow_domains.push(domain.into());
        self
    }
    /// Request the given scope from the oidc provider.
    pub fn scope(mut self, scope: impl Into<String>) -> Self {
        self.scopes.push(scope.into());
        self
    }
}

// transform into the wire protocol format
impl From<OidcOptions> for Oidc {
    fn from(o: OidcOptions) -> Self {
        Oidc {
            issuer_url: o.issuer_url,
            client_id: o.client_id,
            client_secret: o.client_secret,
            sealed_client_secret: Default::default(), // unused in this context
            allow_emails: o.allow_emails,
            allow_domains: o.allow_domains,
            scopes: o.scopes,
        }
    }
}
