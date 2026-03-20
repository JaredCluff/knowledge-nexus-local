use std::sync::Arc;

use anyhow::Result;

use crate::db::{Conversation, Database, Message};

pub struct ConversationService {
    db: Arc<Database>,
}

impl ConversationService {
    pub fn new(db: Arc<Database>) -> Self {
        Self { db }
    }

    pub fn create(&self, conv: &Conversation) -> Result<()> {
        self.db.create_conversation(conv)
    }

    pub fn get(&self, id: &str) -> Result<Option<Conversation>> {
        self.db.get_conversation(id)
    }

    pub fn list_for_user(&self, user_id: &str) -> Result<Vec<Conversation>> {
        self.db.list_conversations_for_user(user_id)
    }

    pub fn add_message(&self, msg: &Message) -> Result<()> {
        self.db.create_message(msg)
    }

    pub fn get_messages(&self, conversation_id: &str) -> Result<Vec<Message>> {
        self.db.list_messages_for_conversation(conversation_id)
    }
}
