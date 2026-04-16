use std::sync::Arc;

use anyhow::Result;

use crate::store::{Conversation, Store, Message};

pub struct ConversationService {
    db: Arc<dyn Store>,
}

impl ConversationService {
    pub fn new(db: Arc<dyn Store>) -> Self {
        Self { db }
    }

    pub async fn create(&self, conv: &Conversation) -> Result<()> {
        self.db.create_conversation(conv).await
    }

    pub async fn get(&self, id: &str) -> Result<Option<Conversation>> {
        self.db.get_conversation(id).await
    }

    pub async fn list_for_user(&self, user_id: &str) -> Result<Vec<Conversation>> {
        self.db.list_conversations_for_user(user_id).await
    }

    pub async fn add_message(&self, msg: &Message) -> Result<()> {
        self.db.create_message(msg).await
    }

    pub async fn get_messages(&self, conversation_id: &str) -> Result<Vec<Message>> {
        self.db.list_messages_for_conversation(conversation_id).await
    }
}
