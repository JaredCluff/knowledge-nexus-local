//! Axum handlers for /v1/memory/* (P9).
//!
//! DTOs are defined in the lib crate at knowledge_nexus_agent::api::memory
//! and are imported here for use by the handlers. Handlers live in this
//! binary-crate module so they can reference K2KServerState directly.
//!
//! Five endpoints mapped to Lin et al.'s 6-phase memory lifecycle:
//! - POST /v1/memory/observe → Write
//! - POST /v1/memory/recall  → Retrieve
//! - POST /v1/memory/reflect → Store (consolidation)
//! - GET  /v1/memory/timeline → Retrieve
//! - POST /v1/memory/forget  → Forget/Rollback

use axum::extract::{Json, Query, State};
use axum::http::StatusCode;
use std::sync::Arc;
use tracing::warn;

use super::server::K2KServerState;
use crate::k2k::models::K2KResult;
use crate::store::{Article, Tier};

use knowledge_nexus_agent::api::bundler::BundledItem;
use knowledge_nexus_agent::api::memory::{
    ForgetRequest, ForgetResponse, ObserveRequest, ObserveResponse, RecallRequest, RecallResponse,
    ReflectRequest, ReflectResponse, TimelineEvent, TimelineQuery, TimelineResponse,
};

/// POST /v1/memory/observe
pub async fn observe(
    State(state): State<Arc<K2KServerState>>,
    Json(req): Json<ObserveRequest>,
) -> Result<Json<ObserveResponse>, (StatusCode, String)> {
    if req.text.trim().is_empty() {
        return Err((StatusCode::BAD_REQUEST, "text is required".into()));
    }

    let owner = state
        .db
        .get_owner_user()
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, format!("owner lookup: {}", e)))?
        .ok_or((StatusCode::INTERNAL_SERVER_ERROR, "No owner user".into()))?;

    let stores = state
        .db
        .list_stores_for_user(&owner.id)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, format!("stores: {}", e)))?;
    let store = stores
        .first()
        .ok_or((StatusCode::INTERNAL_SERVER_ERROR, "No knowledge stores".into()))?;

    let now = chrono::Utc::now().to_rfc3339();
    let memory_id = req.idempotency_key.clone().unwrap_or_else(|| {
        format!(
            "observe-{}",
            chrono::Utc::now()
                .timestamp_nanos_opt()
                .unwrap_or(0)
        )
    });

    let article = Article {
        id: memory_id.clone(),
        store_id: store.id.clone(),
        title: req.text.chars().take(80).collect(),
        content: req.text.clone(),
        source_type: req.modality.clone().unwrap_or_else(|| "user".into()),
        source_id: req.source.clone().unwrap_or_default(),
        content_hash: String::new(), // ArticleService::create computes this
        tags: serde_json::json!([]),
        embedded_at: None,
        created_at: req.ts.clone().unwrap_or_else(|| now.clone()),
        updated_at: now,
        reflects: vec![],
        access_count: 0,
        last_accessed_at: String::new(),
        importance_score: 0.5,
        tier: Tier::Hot,
        pinned: false,
        compacted_into: None,
    };

    match state
        .article_service
        .create(&article, &store.lancedb_collection)
        .await
    {
        Ok(crate::knowledge::articles::CreateResult::Created) => Ok(Json(ObserveResponse {
            memory_id,
            accepted: true,
            reflections_triggered: vec![],
        })),
        Ok(crate::knowledge::articles::CreateResult::Duplicate { existing_id }) => Err((
            StatusCode::CONFLICT,
            format!("duplicate of existing article: {}", existing_id),
        )),
        Err(e) => {
            warn!("observe failed: {}", e);
            Err((StatusCode::INTERNAL_SERVER_ERROR, format!("ingest failed: {}", e)))
        }
    }
}

/// POST /v1/memory/recall
pub async fn recall(
    State(state): State<Arc<K2KServerState>>,
    Json(req): Json<RecallRequest>,
) -> Result<Json<RecallResponse>, (StatusCode, String)> {
    if req.query.trim().is_empty() {
        return Err((StatusCode::BAD_REQUEST, "query is required".into()));
    }

    let owner = state
        .db
        .get_owner_user()
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, format!("owner lookup: {}", e)))?
        .ok_or((StatusCode::INTERNAL_SERVER_ERROR, "No owner user".into()))?;

    let store_filter = req.scope.as_deref();

    let response = state
        .router
        .route(&req.query, &owner.id, store_filter, 50)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, format!("recall failed: {}", e)))?;

    let budget = req
        .token_budget
        .unwrap_or(state.config.agent_api.default_token_budget)
        .min(state.config.agent_api.max_token_budget);

    let bundled = pack_results_to_budget(response.results, budget);

    let follow_ups = bundled
        .items
        .iter()
        .take(3)
        .map(|i| format!("more like '{}'", i.title.chars().take(40).collect::<String>()))
        .collect();

    Ok(Json(RecallResponse {
        items: bundled.items,
        total_budget_used: bundled.total_budget_used,
        items_dropped: bundled.items_dropped,
        items_truncated: bundled.items_truncated,
        follow_ups,
    }))
}

/// POST /v1/memory/reflect
pub async fn reflect(
    State(state): State<Arc<K2KServerState>>,
    Json(req): Json<ReflectRequest>,
) -> Result<Json<ReflectResponse>, (StatusCode, String)> {
    use crate::knowledge::reflection::{ReflectionCluster, Reflector};

    let owner = state
        .db
        .get_owner_user()
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, format!("owner: {}", e)))?
        .ok_or((StatusCode::INTERNAL_SERVER_ERROR, "No owner".into()))?;

    let stores = state
        .db
        .list_stores_for_user(&owner.id)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, format!("stores: {}", e)))?;
    let store = stores
        .first()
        .ok_or((StatusCode::INTERNAL_SERVER_ERROR, "No stores".into()))?;

    let entities = state
        .db
        .list_entities_for_store(&store.id)
        .await
        .unwrap_or_default();

    let reflector = Reflector::new(state.config.extraction.clone());
    let mut reflection_ids = Vec::new();
    let mut clusters_skipped = 0usize;

    for entity in &entities {
        let articles = state
            .db
            .list_articles_for_entity(&entity.id)
            .await
            .unwrap_or_default();

        if articles.len() < req.min_cluster_size {
            clusters_skipped += 1;
            continue;
        }

        let cluster = ReflectionCluster {
            sources: articles.clone(),
            intent: format!("shared entity: {} ({})", entity.name, entity.entity_type),
        };

        match reflector.reflect(&cluster).await {
            Ok(Some(result)) => {
                if req.dry_run {
                    reflection_ids.push(format!("dry-run-{}", entity.id));
                    continue;
                }
                let now = chrono::Utc::now().to_rfc3339();
                let reflection_id = format!(
                    "reflection-api-{}-{}",
                    entity.id,
                    chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0)
                );
                let reflection_article = Article {
                    id: reflection_id.clone(),
                    store_id: store.id.clone(),
                    title: format!("Reflection: {}", entity.name),
                    content: result.delta_summary.clone(),
                    source_type: "reflection".into(),
                    source_id: String::new(),
                    content_hash: format!("refl-{}", reflection_id),
                    tags: serde_json::json!([]),
                    embedded_at: None,
                    created_at: now.clone(),
                    updated_at: now,
                    reflects: result.source_ids,
                    access_count: 0,
                    last_accessed_at: String::new(),
                    importance_score: result.raw_confidence,
                    tier: Tier::Hot,
                    pinned: false,
                    compacted_into: None,
                };
                if let Err(e) = state.db.create_article(&reflection_article).await {
                    warn!("Failed to store reflection: {}", e);
                    continue;
                }
                reflection_ids.push(reflection_id);
            }
            Ok(None) => {}
            Err(e) => warn!("Reflection failed for entity {}: {}", entity.name, e),
        }
    }

    Ok(Json(ReflectResponse {
        reflections_generated: reflection_ids,
        clusters_skipped,
    }))
}

/// GET /v1/memory/timeline
pub async fn timeline(
    State(state): State<Arc<K2KServerState>>,
    Query(q): Query<TimelineQuery>,
) -> Result<Json<TimelineResponse>, (StatusCode, String)> {
    let owner = state
        .db
        .get_owner_user()
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, format!("owner: {}", e)))?
        .ok_or((StatusCode::INTERNAL_SERVER_ERROR, "No owner".into()))?;

    let stores = state
        .db
        .list_stores_for_user(&owner.id)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, format!("stores: {}", e)))?;
    let store = stores
        .first()
        .ok_or((StatusCode::INTERNAL_SERVER_ERROR, "No stores".into()))?;

    let events = state
        .db
        .list_events_for_store(&store.id)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, format!("events: {}", e)))?;

    let filtered: Vec<TimelineEvent> = events
        .into_iter()
        .filter(|e| q.since.as_deref().map_or(true, |s| e.started_at.as_str() >= s))
        .filter(|e| q.until.as_deref().map_or(true, |u| e.ended_at.as_str() <= u))
        .take(q.limit)
        .map(|e| TimelineEvent {
            event_id: e.id,
            title: e.title,
            started_at: e.started_at,
            ended_at: e.ended_at,
            participants: e
                .participants
                .as_array()
                .map(|arr| {
                    arr.iter()
                        .filter_map(|v| v.as_str().map(|s| s.to_string()))
                        .collect()
                })
                .unwrap_or_default(),
            evidence_count: 0,
        })
        .collect();

    Ok(Json(TimelineResponse { events: filtered }))
}

/// POST /v1/memory/forget — soft-archive only, no hard delete via API.
pub async fn forget(
    State(state): State<Arc<K2KServerState>>,
    Json(req): Json<ForgetRequest>,
) -> Result<Json<ForgetResponse>, (StatusCode, String)> {
    if req.reason.trim().is_empty() {
        return Err((StatusCode::BAD_REQUEST, "reason is required".into()));
    }

    match state.db.get_article(&req.memory_id).await {
        Ok(Some(_)) => {}
        Ok(None) => {
            return Err((
                StatusCode::NOT_FOUND,
                format!("Article {} not found", req.memory_id),
            ))
        }
        Err(e) => return Err((StatusCode::INTERNAL_SERVER_ERROR, format!("lookup: {}", e))),
    }

    let reason = format!("forget_api: {}", req.reason);
    state
        .db
        .set_article_tier(&req.memory_id, Tier::Archive, &reason)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, format!("archive: {}", e)))?;

    Ok(Json(ForgetResponse {
        archived: true,
        audit_id: format!(
            "audit-forget-{}",
            chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0)
        ),
    }))
}

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

struct PackedResults {
    items: Vec<BundledItem>,
    total_budget_used: u32,
    items_dropped: u32,
    items_truncated: u32,
}

/// Pack binary-crate K2KResult vec into BundledItems within the token budget.
/// Mirrors the lib's bundler::pack_to_budget but operates on the binary's K2KResult type.
fn pack_results_to_budget(results: Vec<K2KResult>, budget_tokens: u32) -> PackedResults {
    if results.is_empty() || budget_tokens == 0 {
        return PackedResults {
            items: vec![],
            total_budget_used: 0,
            items_dropped: results.len() as u32,
            items_truncated: 0,
        };
    }

    let budget_chars = budget_tokens.saturating_mul(4) as usize;
    let mut items = Vec::new();
    let mut used_chars: usize = 0;
    let mut items_dropped: u32 = 0;
    let mut items_truncated: u32 = 0;

    for r in results {
        let item_chars = r.title.chars().count() + r.summary.chars().count();
        let remaining = budget_chars.saturating_sub(used_chars);

        if remaining == 0 {
            items_dropped += 1;
            continue;
        }

        if item_chars <= remaining {
            items.push(BundledItem {
                article_id: r.article_id,
                title: r.title,
                summary: r.summary,
                confidence: r.confidence,
                truncated: false,
            });
            used_chars += item_chars;
        } else if items.is_empty() {
            let title_chars = r.title.chars().count();
            let remaining_for_summary = remaining.saturating_sub(title_chars);
            let truncated_summary = r.summary.chars().take(remaining_for_summary).collect();
            items.push(BundledItem {
                article_id: r.article_id,
                title: r.title,
                summary: truncated_summary,
                confidence: r.confidence,
                truncated: true,
            });
            items_truncated += 1;
            used_chars = budget_chars;
        } else {
            items_dropped += 1;
        }
    }

    PackedResults {
        items,
        total_budget_used: ((used_chars + 3) / 4) as u32,
        items_dropped,
        items_truncated,
    }
}
