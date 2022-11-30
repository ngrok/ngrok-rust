use crate::mw::middleware_configuration::WebhookVerification as WebhookProto;

/// Configuration for webhook verification.
pub(crate) struct WebhookVerification {
    /// The webhook provider
    pub(crate) provider: String,
    /// The secret for verifying webhooks from this provider.
    pub(crate) secret: String,
}

impl WebhookVerification {}

// transform into the wire protocol format
impl From<&WebhookVerification> for WebhookProto {
    fn from(wv: &WebhookVerification) -> Self {
        WebhookProto {
            provider: wv.provider.clone(),
            secret: wv.secret.clone(),
            sealed_secret: Vec::new(), // unused in this context
        }
    }
}
