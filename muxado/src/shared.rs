use std::sync::Arc;

use async_trait::async_trait;
use futures::lock::Mutex;

use crate::{
    errors::ErrorType,
    session::Session,
    stream::Stream,
};

#[async_trait]
impl<S> Session for Arc<Mutex<S>>
where
    S: Session,
{
    async fn accept(&mut self) -> Option<Stream> {
        self.lock().await.accept().await
    }
    async fn open(&mut self) -> Result<Stream, ErrorType> {
        self.lock().await.open().await
    }
}
