use crate::internals::proto::Oauth;

/// Oauth Options configuration
#[derive(Clone, Default)]
pub struct OauthOptions {
    /// The OAuth provider to use
    provider: String,
    /// Email addresses of users to authorize.
    allow_emails: Vec<String>,
    /// Email domains of users to authorize.
    allow_domains: Vec<String>,
    /// OAuth scopes to request from the provider.
    scopes: Vec<String>,
}

impl OauthOptions {
    /// Create a new [OauthOptions] for the given provider.
    pub fn new(provider: impl Into<String>) -> Self {
        OauthOptions {
            provider: provider.into(),
            ..Default::default()
        }
    }

    /// Allow the oauth user with the given email to access the tunnel.
    pub fn allow_email(mut self, email: impl Into<String>) -> Self {
        self.allow_emails.push(email.into());
        self
    }
    /// Allow the oauth user with the given email domain to access the tunnel.
    pub fn allow_domain(mut self, domain: impl Into<String>) -> Self {
        self.allow_domains.push(domain.into());
        self
    }
    /// Request the given scope from the oauth provider.
    pub fn scope(mut self, scope: impl Into<String>) -> Self {
        self.scopes.push(scope.into());
        self
    }
}

// transform into the wire protocol format
impl From<OauthOptions> for Oauth {
    fn from(o: OauthOptions) -> Self {
        Oauth {
            provider: o.provider,
            client_id: Default::default(),     // unused in this context
            client_secret: Default::default(), // unused in this context
            sealed_client_secret: Default::default(), // unused in this context
            allow_emails: o.allow_emails,
            allow_domains: o.allow_domains,
            scopes: o.scopes,
        }
    }
}
