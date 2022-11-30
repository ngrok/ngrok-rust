use crate::mw::middleware_configuration::OAuth;

/// Oauth Options configuration
pub trait OauthOptionsTrait {
    fn provider(&self) -> String;
    fn allow_emails(&self) -> Vec<String>;
    fn allow_domains(&self) -> Vec<String>;
    fn scopes(&self) -> Vec<String>;
}

#[derive(Default)]
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
    pub fn new(provider: impl Into<String>) -> Self {
        OauthOptions {
            provider: provider.into(),
            ..Default::default()
        }
    }

    pub fn with_allow_oauth_email(&mut self, email: impl Into<String>) -> &mut Self {
        self.allow_emails.push(email.into());
        self
    }
    pub fn with_allow_oauth_domain(&mut self, domain: impl Into<String>) -> &mut Self {
        self.allow_domains.push(domain.into());
        self
    }
    pub fn with_oauth_scope(&mut self, scope: impl Into<String>) -> &mut Self {
        self.scopes.push(scope.into());
        self
    }
}

impl OauthOptionsTrait for OauthOptions {
    fn provider(&self) -> String {
        self.provider.clone()
    }
    fn allow_emails(&self) -> Vec<String> {
        self.allow_emails.clone()
    }
    fn allow_domains(&self) -> Vec<String> {
        self.allow_domains.clone()
    }
    fn scopes(&self) -> Vec<String> {
        self.scopes.clone()
    }
}

// delegate references
impl<'a, T> OauthOptionsTrait for &'a T
where
    T: OauthOptionsTrait,
{
    fn provider(&self) -> String {
        (**self).provider()
    }
    fn allow_emails(&self) -> Vec<String> {
        (**self).allow_emails()
    }
    fn allow_domains(&self) -> Vec<String> {
        (**self).allow_domains()
    }
    fn scopes(&self) -> Vec<String> {
        (**self).scopes()
    }
}

// delegate mutable references
impl<'a, T> OauthOptionsTrait for &'a mut T
where
    T: OauthOptionsTrait,
{
    fn provider(&self) -> String {
        (**self).provider()
    }
    fn allow_emails(&self) -> Vec<String> {
        (**self).allow_emails()
    }
    fn allow_domains(&self) -> Vec<String> {
        (**self).allow_domains()
    }
    fn scopes(&self) -> Vec<String> {
        (**self).scopes()
    }
}

pub(crate) fn to_proto_config(oauth: &Option<Box<dyn OauthOptionsTrait>>) -> Option<OAuth> {
    oauth.as_ref().map(|o| OAuth {
        provider: o.provider(),
        client_id: Default::default(),     // unused in this context
        client_secret: Default::default(), // unused in this context
        sealed_client_secret: Default::default(), // unused in this context
        allow_emails: o.allow_emails(),
        allow_domains: o.allow_domains(),
        scopes: o.scopes(),
    })
}
