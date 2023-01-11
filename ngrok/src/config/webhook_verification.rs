use crate::internals::proto::{
    SecretString,
    WebhookVerification as WebhookProto,
};

/// Configuration for webhook verification.
#[derive(Clone)]
pub(crate) struct WebhookVerification {
    /// The webhook provider
    pub(crate) provider: String,
    /// The secret for verifying webhooks from this provider.
    pub(crate) secret: SecretString,
}

impl WebhookVerification {}

// transform into the wire protocol format
impl From<WebhookVerification> for WebhookProto {
    fn from(wv: WebhookVerification) -> Self {
        WebhookProto {
            provider: wv.provider,
            secret: wv.secret,
            sealed_secret: vec![], // unused in this context
        }
    }
}
