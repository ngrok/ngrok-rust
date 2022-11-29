use crate::mw::middleware_configuration::WebhookVerification as WebhookProto;

/// Configuration for webhook verification.
pub(crate) struct WebhookVerification {
    /// The webhook provider
    pub(crate) provider: String,
    /// The secret for verifying webhooks from this provider.
    pub(crate) secret: String,
}

impl WebhookVerification {}

pub(crate) fn to_proto_config(
    webhook_verification: &Option<WebhookVerification>,
) -> Option<WebhookProto> {
    webhook_verification.as_ref().map(|w| WebhookProto {
        provider: w.provider.clone(),
        secret: w.secret.clone(),
        sealed_secret: Vec::new(),
    })
}
