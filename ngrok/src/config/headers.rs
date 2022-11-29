use std::collections::HashMap;

use crate::mw::middleware_configuration::Headers as HeaderProto;

/// HTTP Headers to modify at the ngrok edge.
#[derive(Default)]
pub(crate) struct Headers {
    /// Headers to add to requests or responses at the ngrok edge.
    added: HashMap<String, String>,
    /// Header names to remove from requests or responses at the ngrok edge.
    removed: Vec<String>,
}

impl Headers {
    pub(crate) fn add(&mut self, name: impl Into<String>, value: impl Into<String>) {
        self.added.insert(name.into(), value.into());
    }
    pub(crate) fn remove(&mut self, name: impl Into<String>) {
        self.removed.push(name.into());
    }

    // transform into the wire protocol format
    pub(crate) fn to_proto_config(&self) -> Option<HeaderProto> {
        if self.added.is_empty() && self.removed.is_empty() {
            return None;
        }
        Some(HeaderProto {
            add: self
                .added
                .clone()
                .into_iter()
                .map(|a| format!("{}:{}", a.0, a.1))
                .collect(),
            remove: self.removed.clone(),
            add_parsed: HashMap::new(), // appears unused
        })
    }
}
