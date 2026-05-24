use anyhow::Result;
use async_trait::async_trait;
use tokio::sync::mpsc::Sender;

use super::types::{InboundMessage, OutboundMessage};

#[async_trait]
pub trait ImChannel: Send + Sync {
    async fn start(&self, tx: Sender<InboundMessage>) -> Result<()>;

    async fn send(&self, message: OutboundMessage) -> Result<()>;
}
