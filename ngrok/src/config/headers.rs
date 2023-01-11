use std::collections::HashMap;

use crate::internals::proto::Headers as HeaderProto;

/// HTTP Headers to modify at the ngrok edge.
#[derive(Clone, Default)]
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
    pub(crate) fn has_entries(&self) -> bool {
        !self.added.is_empty() || !self.removed.is_empty()
    }
}

// transform into the wire protocol format
impl From<Headers> for HeaderProto {
    fn from(headers: Headers) -> Self {
        HeaderProto {
            add: headers
                .added
                .iter()
                .map(|a| format!("{}:{}", a.0, a.1))
                .collect(),
            remove: headers.removed,
            add_parsed: HashMap::new(), // unused in this context
        }
    }
}
