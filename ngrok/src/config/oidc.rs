use crate::mw::middleware_configuration::Oidc;

/// Oidc Options configuration
pub trait OidcOptionsTrait {
    fn issuer_url(&self) -> String;
    fn client_id(&self) -> String;
    fn client_secret(&self) -> String;
    fn allow_emails(&self) -> Vec<String>;
    fn allow_domains(&self) -> Vec<String>;
    fn scopes(&self) -> Vec<String>;
}
#[derive(Default)]
pub struct OidcOptions {
    issuer_url: String,
    client_id: String,
    client_secret: String,
    allow_emails: Vec<String>,
    allow_domains: Vec<String>,
    scopes: Vec<String>,
}

impl OidcOptions {
    pub fn new(
        issuer_url: impl Into<String>,
        client_id: impl Into<String>,
        client_secret: impl Into<String>,
    ) -> Self {
        OidcOptions {
            issuer_url: issuer_url.into(),
            client_id: client_id.into(),
            client_secret: client_secret.into(),
            ..Default::default()
        }
    }

    pub fn with_allow_oidc_email(&mut self, email: impl Into<String>) -> &mut Self {
        self.allow_emails.push(email.into());
        self
    }
    pub fn with_allow_oidc_domain(&mut self, domain: impl Into<String>) -> &mut Self {
        self.allow_domains.push(domain.into());
        self
    }
    pub fn with_oidc_scope(&mut self, scope: impl Into<String>) -> &mut Self {
        self.scopes.push(scope.into());
        self
    }
}

impl OidcOptionsTrait for OidcOptions {
    fn issuer_url(&self) -> String {
        self.issuer_url.clone()
    }
    fn client_id(&self) -> String {
        self.client_id.clone()
    }
    fn client_secret(&self) -> String {
        self.client_secret.clone()
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
impl<'a, T> OidcOptionsTrait for &'a T
where
    T: OidcOptionsTrait,
{
    fn issuer_url(&self) -> String {
        (**self).issuer_url()
    }
    fn client_id(&self) -> String {
        (**self).client_id()
    }
    fn client_secret(&self) -> String {
        (**self).client_secret()
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
impl<'a, T> OidcOptionsTrait for &'a mut T
where
    T: OidcOptionsTrait,
{
    fn issuer_url(&self) -> String {
        (**self).issuer_url()
    }
    fn client_id(&self) -> String {
        (**self).client_id()
    }
    fn client_secret(&self) -> String {
        (**self).client_secret()
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

// transform into the wire protocol format
impl From<&Box<dyn OidcOptionsTrait>> for Oidc {
    fn from(o: &Box<dyn OidcOptionsTrait>) -> Self {
        Oidc {
            issuer_url: o.issuer_url(),
            client_id: o.client_id(),
            client_secret: o.client_secret(),
            sealed_client_secret: Default::default(), // unused in this context
            allow_emails: o.allow_emails(),
            allow_domains: o.allow_domains(),
            scopes: o.scopes(),
        }
    }
}
