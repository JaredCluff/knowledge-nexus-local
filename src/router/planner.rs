use std::sync::Arc;

use anyhow::Result;

use crate::db::{Database, KnowledgeStore};

pub struct QueryPlanner {
    db: Arc<Database>,
}

impl QueryPlanner {
    pub fn new(db: Arc<Database>) -> Self {
        Self { db }
    }

    /// Select which stores to query based on scope and user permissions
    pub fn plan(&self, user_id: &str, scope: &str) -> Result<Vec<KnowledgeStore>> {
        match scope {
            "personal" => {
                // Only the user's personal stores
                let stores = self.db.list_stores_for_user(user_id)?;
                let personal: Vec<_> = stores
                    .into_iter()
                    .filter(|s| s.store_type == "personal")
                    .collect();
                if personal.is_empty() {
                    // Fallback to all user stores
                    self.db.list_stores_for_user(user_id)
                } else {
                    Ok(personal)
                }
            }
            "family" => {
                // All family + shared stores
                let all_stores = self.db.list_stores()?;
                Ok(all_stores
                    .into_iter()
                    .filter(|s| s.store_type == "family" || s.store_type == "shared")
                    .collect())
            }
            _ => {
                // "all" - query everything accessible
                self.db.list_stores()
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::{Database, KnowledgeStore, User};
    use anyhow::Result;

    fn setup_db() -> Result<Arc<Database>> {
        let db = Arc::new(Database::open_in_memory()?);
        let now = chrono::Utc::now().to_rfc3339();

        db.create_user(&User {
            id: "u1".into(),
            username: "alice".into(),
            display_name: "Alice".into(),
            is_owner: true,
            settings: serde_json::json!({}),
            created_at: now.clone(),
            updated_at: now.clone(),
        })?;

        db.create_store(&KnowledgeStore {
            id: "s1".into(),
            owner_id: "u1".into(),
            store_type: "personal".into(),
            name: "Alice's Notes".into(),
            lancedb_collection: "store_s1".into(),
            created_at: now.clone(),
            updated_at: now.clone(),
        })?;

        db.create_store(&KnowledgeStore {
            id: "s2".into(),
            owner_id: "u1".into(),
            store_type: "family".into(),
            name: "Family Recipes".into(),
            lancedb_collection: "store_s2".into(),
            created_at: now.clone(),
            updated_at: now,
        })?;

        Ok(db)
    }

    #[test]
    fn test_plan_personal() -> Result<()> {
        let db = setup_db()?;
        let planner = QueryPlanner::new(db);
        let stores = planner.plan("u1", "personal")?;
        assert_eq!(stores.len(), 1);
        assert_eq!(stores[0].store_type, "personal");
        Ok(())
    }

    #[test]
    fn test_plan_family() -> Result<()> {
        let db = setup_db()?;
        let planner = QueryPlanner::new(db);
        let stores = planner.plan("u1", "family")?;
        assert_eq!(stores.len(), 1);
        assert_eq!(stores[0].store_type, "family");
        Ok(())
    }

    #[test]
    fn test_plan_all() -> Result<()> {
        let db = setup_db()?;
        let planner = QueryPlanner::new(db);
        let stores = planner.plan("u1", "all")?;
        assert_eq!(stores.len(), 2);
        Ok(())
    }
}
