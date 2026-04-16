use std::sync::Arc;

use anyhow::Result;

use crate::store::{Store, KnowledgeStore};

pub struct QueryPlanner {
    db: Arc<dyn Store>,
}

impl QueryPlanner {
    pub fn new(db: Arc<dyn Store>) -> Self {
        Self { db }
    }

    /// Select which stores to query based on scope and user permissions
    pub async fn plan(&self, user_id: &str, scope: &str) -> Result<Vec<KnowledgeStore>> {
        match scope {
            "personal" => {
                // Only the user's personal stores
                let stores = self.db.list_stores_for_user(user_id).await?;
                let personal: Vec<_> = stores
                    .into_iter()
                    .filter(|s| s.store_type == "personal")
                    .collect();
                if personal.is_empty() {
                    // Fallback to all user stores
                    self.db.list_stores_for_user(user_id).await
                } else {
                    Ok(personal)
                }
            }
            "family" => {
                // All family + shared stores
                let all_stores = self.db.list_stores().await?;
                Ok(all_stores
                    .into_iter()
                    .filter(|s| s.store_type == "family" || s.store_type == "shared")
                    .collect())
            }
            _ => {
                // "all" - query everything accessible
                self.db.list_stores().await
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::{SurrealStore, KnowledgeStore, User};
    use anyhow::Result;

    async fn setup_db() -> Result<Arc<dyn Store>> {
        let store = SurrealStore::open_in_memory().await?;
        let db: Arc<dyn Store> = Arc::new(store);
        let now = chrono::Utc::now().to_rfc3339();

        db.create_user(&User {
            id: "u1".into(),
            username: "alice".into(),
            display_name: "Alice".into(),
            is_owner: true,
            settings: serde_json::json!({}),
            created_at: now.clone(),
            updated_at: now.clone(),
        }).await?;

        db.create_store(&KnowledgeStore {
            id: "s1".into(),
            owner_id: "u1".into(),
            store_type: "personal".into(),
            name: "Alice's Notes".into(),
            lancedb_collection: "store_s1".into(),
            quantizer_version: "ivf_pq_v1".into(),
            created_at: now.clone(),
            updated_at: now.clone(),
        }).await?;

        db.create_store(&KnowledgeStore {
            id: "s2".into(),
            owner_id: "u1".into(),
            store_type: "family".into(),
            name: "Family Recipes".into(),
            lancedb_collection: "store_s2".into(),
            quantizer_version: "ivf_pq_v1".into(),
            created_at: now.clone(),
            updated_at: now,
        }).await?;

        Ok(db)
    }

    #[tokio::test]
    async fn test_plan_personal() -> Result<()> {
        let db = setup_db().await?;
        let planner = QueryPlanner::new(db);
        let stores = planner.plan("u1", "personal").await?;
        assert_eq!(stores.len(), 1);
        assert_eq!(stores[0].store_type, "personal");
        Ok(())
    }

    #[tokio::test]
    async fn test_plan_family() -> Result<()> {
        let db = setup_db().await?;
        let planner = QueryPlanner::new(db);
        let stores = planner.plan("u1", "family").await?;
        assert_eq!(stores.len(), 1);
        assert_eq!(stores[0].store_type, "family");
        Ok(())
    }

    #[tokio::test]
    async fn test_plan_all() -> Result<()> {
        let db = setup_db().await?;
        let planner = QueryPlanner::new(db);
        let stores = planner.plan("u1", "all").await?;
        assert_eq!(stores.len(), 2);
        Ok(())
    }
}
