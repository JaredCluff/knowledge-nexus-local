# K2K Protocol Specification

**Author:** Jared Cluff | **Version:** 1.1 | **Date:** March 2026

---

## 1. Abstract

K2K (Knowledge-to-Knowledge) is a federation protocol that allows autonomous AI agents to discover each other on a local network, authenticate with signed tokens, and route semantic queries across distributed knowledge stores. Each agent indexes its own files and knowledge articles locally. K2K gives any authorized agent the ability to search another agent's knowledge, delegate tasks to remote capabilities, and merge results from multiple sources into a single ranked response. The protocol runs over HTTP with JSON message framing, uses RSA-256 signed JWTs for authentication, and discovers peers via DNS-SD/mDNS.

---

## 2. Motivation

A single AI agent knows what's on a single machine. That's useful, but it falls short the moment your knowledge lives in more than one place -- a laptop, a home server, a family NAS, a cloud-hosted knowledge base.

Federated knowledge routing solves this. Instead of centralizing all data into one store (with the privacy, bandwidth, and staleness problems that brings), K2K lets each node stay sovereign over its own data while participating in a shared query mesh. When you ask "Where's the pasta recipe grandma sent last month?", the query fans out to every node in your federation, each one searches its own index, and the results come back merged and ranked -- as if the knowledge lived in one place.

The design priorities:

- **Data stays local.** Files never leave the machine that indexed them unless a query explicitly requests content. The protocol transmits search results (titles, summaries, confidence scores), not raw documents.
- **Zero configuration networking.** Nodes discover each other via mDNS on the local network. No port forwarding, no DNS records, no central registry.
- **Mutual authentication.** Every query carries an RSA-256 signed JWT. Nodes maintain an explicit allowlist of registered clients. Unregistered clients cannot query.
- **Firewall-friendly.** Agents initiate outbound connections only. The K2K server binds to `127.0.0.1` by default -- it is not exposed to the public internet unless the operator explicitly configures it.

---

## 3. Terminology

| Term | Definition |
|------|-----------|
| **Node** | A single NexiBot instance running the K2K server. Each node has a unique `node_id` (UUID), an RSA key pair, and one or more knowledge stores. |
| **Hub** | An optional central WebSocket relay that nodes can connect to for queries routed over the internet (as opposed to LAN-only mDNS). |
| **K2K Query** | A semantic search request sent from one node (or client) to another. Contains a natural-language query string, optional filters, and authentication credentials. |
| **Knowledge Store** | A named collection of indexed articles or files. Stores have types: `personal`, `family`, or `shared`. A node may host multiple stores. |
| **Knowledge Layer** | The full stack that processes a query: embedding generation, vector search, hybrid keyword search, result merging, and reranking. |
| **Client** | Any agent or application that registers its public key with a node and authenticates via JWT to query that node's knowledge. |
| **Federation Agreement** | A record linking a local store to a remote node, granting that node read (or read/write) access to the store's data. |
| **Capability** | A named function a node can perform -- for example, `semantic_search`, `web_search`, or a remote microservice endpoint. Capabilities are advertised and can be invoked by other nodes via task delegation. |
| **Task** | An asynchronous unit of work submitted to a node's capability. Tasks have a lifecycle: `queued` -> `running` -> `completed` (or `failed` / `cancelled`). |
| **RRF** | Reciprocal Rank Fusion. The algorithm used to merge ranked result lists from multiple stores into a single ranking. |
| **Provenance** | Metadata attached to each search result indicating which store it came from, its original rank in that store, and its RRF score after merging. |

---

## 4. Protocol Overview

A K2K interaction follows this sequence:

1. **Discovery.** Nodes advertise themselves via mDNS using the service type `_k2k._tcp.local.`. Other nodes on the same network segment discover them, validate against a trusted-node allowlist, and register them in a local node registry.

2. **Client Registration.** Before a client can query a node, it registers its RSA public key at the node's `/k2k/v1/register-client` endpoint. The node stores the key and sets the client's status to `pending`. An admin approves or rejects the client (or the node auto-approves if a pre-shared registration secret matches).

3. **Authentication.** For every protected request, the client generates a short-lived RS256 JWT (5-minute TTL by default), signs it with its RSA private key, and sends it as a Bearer token. The node verifies the signature against the registered public key, checks the client's approval status, and validates expiration.

4. **Query Routing.** The client sends a query to `/k2k/v1/query` (direct search) or `/k2k/v1/route` (smart routing). Smart routing classifies the query by scope (personal, family, or all), selects the appropriate stores, runs vector + keyword hybrid search, optionally fans out to federated remote nodes, merges results with RRF, reranks, and returns.

5. **Task Delegation.** For capabilities beyond search, a client submits a task to `/k2k/v1/tasks`. The node queues the task, executes it via the appropriate handler (local or remote), and streams progress updates over SSE.

6. **Capability Advertisement.** Each node publishes a manifest at `/.well-known/k2k-manifest` describing its capabilities. Other nodes periodically fetch these manifests to discover what a peer can do.

```
 Node A                                    Node B
   |                                          |
   |--- mDNS announce: _k2k._tcp.local. ---->|
   |<--- mDNS announce: _k2k._tcp.local. ----|
   |                                          |
   |--- POST /k2k/v1/register-client ------->|
   |<-- 200 { status: "pending" } ------------|
   |                                          |
   | (admin approves on Node B)               |
   |                                          |
   |--- POST /k2k/v1/query ----------------->|
   |    Authorization: Bearer <RS256 JWT>     |
   |    { "query": "pasta recipe", ... }      |
   |<-- 200 { results: [...], ... } ----------|
```

---

## 5. Transport

### 5.1 HTTP API (Primary)

The K2K server is an HTTP/1.1 server built on Axum. It binds to `127.0.0.1:{port}` (default port configured per-node, typically 19850).

All request and response bodies are JSON (`Content-Type: application/json`).

Clients MUST include the header `X-K2K-Protocol-Version: 1.1` on all authenticated requests.

**Version negotiation:** Nodes MUST ignore unknown fields in messages to maintain forward compatibility. Nodes SHOULD respond with the highest protocol version they support that is less than or equal to the version the client requested. If a node receives a request with a protocol version it does not recognize, it SHOULD respond using the highest version it supports and include the `X-K2K-Protocol-Version` header in the response indicating the version used.

### 5.2 WebSocket (Hub Connection)

For internet-reachable federation (optional), nodes maintain a persistent WebSocket connection to a central Hub. The WebSocket transport uses JSON-framed message envelopes:

```json
{
  "type": "query_request",
  "payload": { ... },
  "timestamp": "2026-03-18T12:00:00Z"
}
```

Message types carried over WebSocket:

| Type | Direction | Description |
|------|-----------|-------------|
| `auth_request` | Node -> Hub | Initial authentication with JWT + device info |
| `auth_response` | Hub -> Node | Session ID, heartbeat interval |
| `heartbeat` | Node -> Hub | Periodic keep-alive with agent metrics |
| `heartbeat_ack` | Hub -> Node | Server acknowledgment |
| `query_request` | Hub -> Node | Semantic search query from another node |
| `query_response` | Node -> Hub | Search results |
| `chat_message` | Hub -> Node | Chat-style query with context |
| `chat_response` | Node -> Hub | Chat response with source citations |
| `error` | Either | Error with code and message |
| `disconnect` | Either | Graceful disconnection with reason |

The WebSocket connection uses exponential backoff for reconnection (base delay configurable, 10% jitter, capped at max delay). Connection and authentication both have 30-second timeouts.

### 5.3 SSE (Server-Sent Events)

Task progress is streamed over SSE at `/k2k/v1/tasks/events`. Events are filtered per-client -- a subscriber only receives events for tasks they submitted. Event types:

- `status_changed` -- task entered a new state
- `progress` -- percentage update (0-100)
- `completed` -- task finished with result
- `failed` -- task failed with error message

### 5.4 TLS

For production deployments and all remote capability endpoints, HTTPS is required. The `RemoteCapabilityHandler` rejects any endpoint URL that does not begin with `https://`. Remote nodes discovered via mDNS that advertise non-HTTPS endpoints are skipped.

On a trusted local network (127.0.0.1 binding), plain HTTP is acceptable because traffic never leaves the machine.

### 5.5 Future Transport Bindings

HTTP/1.1 is the primary transport binding defined by this specification. WebSocket (Section 5.2) and SSE (Section 5.3) are secondary bindings for specific use cases. Future versions of this specification MAY define bindings for gRPC (for high-throughput inter-datacenter links) and QUIC (for low-latency mobile clients). Implementations MUST support the HTTP binding; other bindings are OPTIONAL.

---

## 6. Authentication

### 6.1 Key Generation

Each node generates a 2048-bit RSA key pair on first startup:

- Private key: `{config_dir}/k2k_private_key.pem` (PKCS#8 format, file permissions 0600)
- Public key: `{config_dir}/k2k_public_key.pem` (SPKI format)

Clients also generate their own RSA-2048 key pair using the `generate_rsa_keypair()` utility from `k2k-common`.

### 6.2 Client Registration

A client registers by sending its public key to the node:

```
POST /k2k/v1/register-client
```

```json
{
  "client_id": "my-laptop",
  "client_name": "Jared's MacBook Air",
  "public_key_pem": "-----BEGIN PUBLIC KEY-----\nMIIBIjANBgkq...\n-----END PUBLIC KEY-----",
  "registration_secret": "optional-pre-shared-key"
}
```

Response:

```json
{
  "status": "pending",
  "client_id": "my-laptop",
  "message": "Client registered, pending admin approval"
}
```

Status values: `approved`, `pending`, `rejected`.

If the node has a `registration_secret` configured and the client's secret matches (constant-time comparison), the status is immediately set to `approved`. Otherwise, the client enters `pending` and must be approved via the admin endpoint.

### 6.3 Admin Approval

```
POST /k2k/v1/admin/approve-client
{ "client_id": "my-laptop" }
```

Admin endpoints are only accessible on localhost (the server binds to 127.0.0.1).

### 6.4 JWT Format

Every authenticated request carries a Bearer token:

```
Authorization: Bearer eyJhbGciOiJSUzI1NiIsInR5cCI6IkpXVCJ9...
```

The JWT is signed with the client's RSA private key using RS256 (RSASSA-PKCS1-v1_5 with SHA-256).

**Header:**

```json
{
  "alg": "RS256",
  "typ": "JWT"
}
```

**Claims:**

```json
{
  "iss": "kb:my-laptop",
  "aud": "kb:my-laptop",
  "source_kb_id": "my-laptop",
  "client_id": "my-laptop",
  "iat": 1742284800,
  "exp": 1742285100,
  "jti": "550e8400-e29b-41d4-a716-446655440000",
  "transfer_id": "a1b2c3d4e5f67890"
}
```

| Claim | Description |
|-------|-------------|
| `iss` | Issuer. Format: `kb:{client_id}` |
| `aud` | Audience. Mirrors the issuer. |
| `source_kb_id` | The requesting knowledge base ID. |
| `client_id` | The authenticated client identity. Used for ACL enforcement, rate limiting, and task ownership. |
| `iat` | Issued-at timestamp (Unix seconds). Must not be more than 60 seconds in the future. |
| `exp` | Expiration timestamp. Default TTL is 300 seconds (5 minutes). Configurable for long-running operations. |
| `jti` | JWT ID. UUIDv4 for replay prevention. |
| `transfer_id` | Opaque transfer correlation ID. |

### 6.5 Verification Flow

The authentication middleware performs these steps in order:

1. Extract Bearer token from the `Authorization` header.
2. Decode the JWT payload (without verification) to read `client_id`.
3. Look up the client in the key registry.
4. Check that the client's status is `approved`. Reject `pending` (403) and `rejected` (403).
5. Verify the RS256 signature using the client's registered public key.
6. Validate `exp` -- reject if expired.
7. Validate `iat` -- reject if more than 60 seconds in the future.
8. Check the client against the `allowed_clients` allowlist (if configured).
9. Inject the validated claims into the request context for downstream handlers.

---

## 7. Message Types

### 7.1 Health Check

```
GET /k2k/v1/health
```

No authentication required.

```json
{
  "status": "healthy",
  "node_id": "a1b2c3d4-5678-9abc-def0-1234567890ab",
  "node_type": "system_agent",
  "capabilities": ["semantic_search", "file_access", "knowledge_store", "routing", "web_search"],
  "indexed_files": 14523,
  "uptime_seconds": 86400
}
```

### 7.2 Node Info

```
GET /k2k/v1/info
```

No authentication required. Returns node identity and public key for key exchange.

```json
{
  "node_id": "a1b2c3d4-5678-9abc-def0-1234567890ab",
  "node_name": "Jared's MacBook Air",
  "node_type": "system_agent",
  "version": "0.6.0",
  "public_key": "-----BEGIN PUBLIC KEY-----\nMIIBIjANBgkq...\n-----END PUBLIC KEY-----",
  "federation_endpoint": "http://localhost:19850/k2k/v1"
}
```

Note: security-sensitive fields (allowed paths, blocked patterns) are intentionally omitted from this unauthenticated endpoint to prevent attackers from crafting path-traversal payloads.

### 7.3 Query Request

```
POST /k2k/v1/query
Authorization: Bearer <JWT>
X-K2K-Protocol-Version: 1.1
```

```json
{
  "query": "How do I fix a leaky faucet?",
  "requesting_store": "my-laptop",
  "top_k": 10,
  "filters": {
    "paths": ["/home/user/Documents"],
    "file_types": [".md", ".pdf"],
    "max_file_size_bytes": 10485760
  },
  "context": "home_maintenance",
  "target_stores": ["store-abc", "store-def"],
  "trace_id": "abc123-def456-789"
}
```

| Field | Type | Required | Default | Description |
|-------|------|----------|---------|-------------|
| `query` | string | yes | -- | Natural language search query. Max 10,000 characters. |
| `requesting_store` | string | yes | -- | ID of the requesting knowledge base. |
| `top_k` | integer | no | 10 | Maximum number of results. Capped at 100. |
| `filters` | object | no | null | Path, file type, and size filters. |
| `context` | string | no | null | Contextual hint for result weighting. |
| `target_stores` | string[] | no | null | Restrict search to specific stores. |
| `trace_id` | string | no | null | Distributed tracing correlation ID. See Section 7.11. |

### 7.4 Query Response

```json
{
  "query_id": "f47ac10b-58cc-4372-a567-0e02b2c3d479",
  "results": [
    {
      "article_id": "art-001",
      "store_id": "a1b2c3d4",
      "title": "Plumbing Repair Guide",
      "summary": "Step-by-step instructions for fixing common leaks...",
      "content": "Full article content here...",
      "confidence": 0.87,
      "source_type": "local_file",
      "tags": [],
      "metadata": {
        "path": "/home/user/Documents/home/plumbing.md",
        "size_bytes": 4096,
        "modified_at": "2026-01-15T10:30:00Z",
        "content_type": "text/markdown",
        "document_type": "markdown",
        "chunk_index": 2
      },
      "provenance": {
        "store_id": "store-personal",
        "store_type": "personal",
        "original_rank": 0,
        "rrf_score": 0.0164
      }
    }
  ],
  "total_results": 1,
  "stores_queried": ["store-personal"],
  "query_time_ms": 42,
  "routing_decision": {
    "scope": "all",
    "stores_selected": 2,
    "federation_enabled": true
  },
  "trace_id": "abc123-def456-789"
}
```

The `trace_id` field, if present in the request, MUST be echoed in the response. See Section 7.11 for distributed tracing semantics.

### 7.5 Routed Query

```
POST /k2k/v1/route
Authorization: Bearer <JWT>
```

```json
{
  "query": "What's the family wifi password?",
  "user_id": "user-jared",
  "context": "family",
  "top_k": 5
}
```

The routed query endpoint runs the full routing pipeline: context classification, store selection, hybrid search, federation fan-out, RRF merge, and reranking. The response format is identical to a standard query response, with an additional `routing_decision` field showing which scope was selected and how many stores were queried.

### 7.6 Task Submission

```
POST /k2k/v1/tasks
Authorization: Bearer <JWT>
X-K2K-Protocol-Version: 1.1
```

```json
{
  "capability_id": "semantic_search",
  "input": {
    "query": "machine learning papers",
    "top_k": 5
  },
  "requesting_node_id": "node-abc",
  "timeout_seconds": 60,
  "context": "research",
  "priority": "normal",
  "trace_id": "abc123-def456-789"
}
```

| Field | Type | Required | Default | Description |
|-------|------|----------|---------|-------------|
| `capability_id` | string | yes | -- | The capability to invoke. |
| `input` | object | yes | -- | Input payload for the capability handler. |
| `requesting_node_id` | string | no | null | The originating node's ID. |
| `timeout_seconds` | integer | no | 60 | Maximum execution time. Capped at 300. |
| `context` | string | no | null | Contextual hint for task execution. |
| `priority` | string | no | "normal" | Task priority: `low`, `normal`, `high`. |
| `trace_id` | string | no | null | Distributed tracing correlation ID. See Section 7.11. |

Response (202 Accepted):

```json
{
  "task_id": "task-550e8400-e29b-41d4-a716-446655440000",
  "status": "queued",
  "trace_id": "abc123-def456-789"
}
```

### 7.7 Task Status

```
GET /k2k/v1/tasks/{task_id}
Authorization: Bearer <JWT>
```

```json
{
  "task_id": "task-550e8400",
  "status": "completed",
  "result": {
    "data": { "results": [...], "total": 5 },
    "duration_ms": 234
  },
  "error": null,
  "created_at": "2026-03-18T12:00:00Z",
  "updated_at": "2026-03-18T12:00:01Z",
  "progress": 100,
  "trace_id": "abc123-def456-789"
}
```

Task statuses: `queued`, `running`, `completed`, `failed`, `cancelled`.

The `trace_id` field, if present in the original task submission, MUST be included in the status response. See Section 7.11.

### 7.8 Task Cancellation

```
DELETE /k2k/v1/tasks/{task_id}
Authorization: Bearer <JWT>
```

Returns 204 No Content on success. Returns 409 Conflict if the task is not in a cancellable state (already completed or failed).

### 7.9 Task Events (SSE)

```
GET /k2k/v1/tasks/events
Authorization: Bearer <JWT>
```

Returns a Server-Sent Events stream. Each event is JSON-encoded:

```
event: progress
data: {"task_id":"task-550e8400","client_id":"my-laptop","event_type":"progress","data":{"progress":50},"timestamp":"2026-03-18T12:00:00.500Z"}

event: completed
data: {"task_id":"task-550e8400","client_id":"my-laptop","event_type":"completed","data":{"duration_ms":234},"timestamp":"2026-03-18T12:00:01Z"}
```

### 7.10 Error Response

All error responses share a common format:

```json
{
  "error": "Unauthorized",
  "detail": "JWT verification failed: Signature verification failed",
  "status_code": 401
}
```

Standard error codes:

| Status | Error | When |
|--------|-------|------|
| 400 | Bad Request | Malformed request, missing fields, query too long |
| 401 | Unauthorized | Missing/invalid JWT, unknown client |
| 403 | Forbidden | Client pending/rejected, not in allowlist, path not allowed |
| 404 | Not Found | Resource not found (also used to mask authorization failures) |
| 422 | Unprocessable Entity | Content exceeds size limits, storage quota exceeded |
| 429 | Too Many Requests | Per-client rate limit exceeded. Includes `Retry-After` header. |
| 500 | Internal Server Error | Embedding failure, vector search failure, DB error |

### 7.11 Distributed Tracing

Implementations SHOULD propagate `trace_id` across node boundaries to enable distributed tracing. The `trace_id` field is an optional string present in Query Request (Section 7.3), Query Response (Section 7.4), Task Submission (Section 7.6), and Task Status (Section 7.7).

If a request includes a `trace_id`, the response MUST echo it back. If absent, a node MAY generate one. When a node fans out a query or delegates a task to a remote node, it SHOULD include the original `trace_id` in the outbound request so that the entire request chain can be correlated.

---

## 8. Node Discovery

### 8.1 DNS-SD / mDNS

K2K nodes discover each other using multicast DNS (mDNS) with the service type:

```
_k2k._tcp.local.
```

Each node advertises a DNS-SD service record with these TXT properties:

| Property | Description |
|----------|-------------|
| `version` | Agent version string (e.g., "0.6.0") |
| `node_id` | The node's UUID |
| `scope` | Federation scope (e.g., "family") |

The instance name is derived from the device name, lowercased with spaces replaced by hyphens, truncated to 63 bytes (the RFC 6763 limit for mDNS instance names).

### 8.2 Discovery Flow

1. On startup, a node creates an mDNS daemon and registers its service.
2. The node begins browsing for `_k2k._tcp.local.` services.
3. When a `ServiceResolved` event arrives, the node:
   - Extracts the `node_id` from TXT properties.
   - Skips its own node ID.
   - Checks the `node_id` against `discovery.trusted_node_ids` in the config. **If the discovered node is not in the trusted list, it is ignored.** If the trusted list is empty, all peers are ignored (closed by default).
   - Registers the node in the local SQLite-backed node registry with its host, port, endpoint URL, and capabilities.
4. Periodically (configurable interval), the node runs health checks against all registered nodes by hitting their `/k2k/v1/health` endpoint with a 5-second timeout. Nodes that fail the check are marked unhealthy and excluded from federation queries.

### 8.3 WAN Discovery

For internet-scale discovery beyond the local network, K2K supports three complementary mechanisms:

**DNS SRV Records.** Nodes MAY publish DNS SRV records under `_k2k._tcp.example.com` to advertise their K2K endpoint to the wider internet. The SRV record's target and port fields indicate the node's K2K server address. Operators are responsible for configuring appropriate DNS entries with their DNS provider.

**Bootstrap Nodes.** For environments where DNS SRV is not available, nodes can be configured with a static list of bootstrap node URLs via the `discovery.bootstrap_nodes` configuration key. On startup, the node contacts each bootstrap URL to exchange node information and discover additional peers. Bootstrap nodes are typically well-known, long-lived nodes that serve as entry points to the federation.

**Well-Known Manifest.** The `/.well-known/k2k-manifest` endpoint (defined in Section 9.1) serves as the WebFinger-style discovery mechanism. Remote clients can probe a known hostname at this well-known path to discover the node's capabilities and federation endpoint without prior knowledge of the node's configuration. This enables web-based discovery where a domain name alone is sufficient to locate a K2K node.

### 8.4 Node Registry

The node registry is backed by SQLite. Each discovered node record contains:

| Field | Type | Description |
|-------|------|-------------|
| `node_id` | string | Remote node's UUID |
| `host` | string | IP address or hostname |
| `port` | u16 | K2K server port |
| `endpoint` | string | Full K2K endpoint URL |
| `capabilities` | JSON | List of capability IDs |
| `last_seen` | string | RFC 3339 timestamp |
| `healthy` | bool | Whether the last health check passed |

---

## 9. Capability Advertisement

### 9.1 Well-Known Manifest

Every K2K node serves a machine-readable manifest at:

```
GET /.well-known/k2k-manifest
```

No authentication required. This endpoint is intended for service discovery.

```json
{
  "service_name": "system-agent-a1b2c3d4",
  "version": "0.6.0",
  "capabilities": [
    {
      "id": "semantic_search",
      "name": "Semantic Search",
      "category": "Knowledge",
      "description": "Search indexed files using semantic similarity",
      "min_protocol_version": "1.0",
      "input_schema": {
        "type": "object",
        "properties": {
          "query": { "type": "string", "description": "Search query" },
          "top_k": { "type": "integer", "default": 10 }
        },
        "required": ["query"]
      },
      "output_schema": {
        "type": "object",
        "properties": {
          "results": { "type": "array" },
          "total": { "type": "integer" }
        }
      },
      "endpoint": "/k2k/v1/tasks",
      "method": "POST",
      "supports_streaming": false
    },
    {
      "id": "web_search",
      "name": "Web Search",
      "category": "Tool",
      "description": "Search the web using configured search providers (Brave, Google, Bing)",
      "min_protocol_version": "1.0",
      "input_schema": {
        "type": "object",
        "properties": {
          "query": { "type": "string" },
          "max_results": { "type": "integer", "default": 10 }
        },
        "required": ["query"]
      },
      "endpoint": "/k2k/v1/tasks",
      "method": "POST",
      "supports_streaming": false
    }
  ]
}
```

If a capability requires protocol features introduced after v1.0, the `min_protocol_version` field indicates the minimum K2K protocol version a client must support to invoke it. Clients SHOULD check this field before submitting a task and avoid invoking capabilities whose `min_protocol_version` exceeds the client's supported protocol version.

### 9.2 Capability Categories

| Category | Description |
|----------|-------------|
| `Knowledge` | Capabilities related to search, retrieval, and storage of knowledge articles. |
| `Tool` | Capabilities that perform actions: web search, file access, browser automation. |
| `Skill` | Higher-level composed capabilities (e.g., "summarize a document"). |
| `Compute` | Raw compute tasks (e.g., embedding generation, model inference). |

### 9.3 Capability Sources

Capabilities are registered from three sources, in order of precedence:

1. **Built-in.** Hardcoded defaults: `semantic_search`, `file_access`, `knowledge_store`, `routing`, `web_search`.
2. **Configuration.** Loaded from `capabilities.yaml` at startup (and on hot-reload via `GET /k2k/v1/capabilities/refresh`).
3. **Discovery.** Fetched from remote services' `/.well-known/k2k-manifest` endpoints. Discovered capabilities have a 15-minute TTL and are evicted when stale.

### 9.4 Remote Capability Handlers

When a capability's `handler_type` is `remote`, task inputs are forwarded as an HTTP request to the configured endpoint URL. The handler supports:

- Configurable HTTP method (GET, POST, PUT, PATCH)
- Authentication: none, Bearer token, or API key header
- Input/output field remapping (rename JSON keys between the K2K schema and the remote API's schema)
- Configurable timeout
- HTTPS enforcement (non-HTTPS endpoints are rejected at both registration and execution time)

### 9.5 Auto-Discovery

The `/k2k/v1/discover-services` endpoint probes a list of known service URLs (both configured and hardcoded) for their `/.well-known/k2k-manifest`. For each reachable service:

1. Fetch the manifest.
2. Parse capabilities.
3. Register them in the capability registry.
4. Create remote handlers in the task queue.

Results are cached per the discovery TTL. Services that are still within their TTL are skipped.

### 9.6 Schema Validation

Implementations SHOULD validate remote handler responses against the declared `output_schema` when present. Invalid responses SHOULD be logged and MAY be returned to the caller with a warning flag (e.g., `"schema_valid": false` in the task result metadata). Schema validation helps catch integration errors early and provides operational visibility into remote capability contract drift.

---

## 10. Query Routing

### 10.1 The Routing Pipeline

When a query hits the `/k2k/v1/route` endpoint, it passes through five stages:

```
Query -> Classify -> Plan -> Execute -> Merge -> Rerank -> Response
                                |
                                +--- Federation (parallel) --->
```

**Stage 1: Context Classification.** A regex-based classifier examines the query text for scope indicators:

- Words like "my", "mine", "personal", "private" -> `personal` scope
- Words like "family", "shared", "our", "household", "mom", "dad" -> `family` scope
- No strong signal -> `all` scope

The caller can also supply an explicit scope hint in the `context` field.

**Stage 2: Query Planning.** The planner selects which knowledge stores to query based on the classified scope:

| Scope | Stores Selected |
|-------|----------------|
| `personal` | Only the requesting user's personal stores. Falls back to all user stores if none are personal. |
| `family` | All stores of type `family` or `shared`. |
| `all` | Every store in the database. |

**Stage 3: Query Execution.** For each selected store, the executor:

1. Generates a single embedding from the query text (shared across all stores to avoid redundant computation).
2. Runs a vector similarity search against the store's LanceDB collection.
3. If a hybrid searcher is available, also runs a BM25 keyword search via SQLite FTS5.
4. Merges vector and keyword results per-store using a hybrid scoring function.

In parallel, if federation is enabled and healthy remote nodes exist, the executor also queries each remote node's `/route` endpoint and collects their results.

**Stage 4: Result Merging (RRF).** Results from all stores (local and remote) are merged using Reciprocal Rank Fusion with constant k=60:

```
RRF_score(document) = SUM over all stores: 1 / (60 + rank_in_store + 1)
```

Documents that appear in multiple stores accumulate higher RRF scores. Duplicates (same `article_id`) are collapsed, keeping the version with the highest individual confidence score.

**Stage 5: Reranking.** The merged results are reranked with boosts for:

- Title matches (exact phrase present in result title)
- Phrase proximity (query terms appear close together in content)
- Recency (recently modified files get a small boost)

### 10.2 Low-Confidence Retry

After reranking, if all top results have low confidence (below an internal threshold relative to the query length), the router attempts query expansion. It generates an alternative phrasing of the query and re-executes the full pipeline. If the expanded query produces better results, those are returned instead.

### 10.3 Provenance Tracking

Every result carries a `provenance` object:

```json
{
  "store_id": "store-personal",
  "store_type": "personal",
  "original_rank": 0,
  "rrf_score": 0.0164
}
```

This tells the caller exactly where each result came from and how it scored, enabling transparency about which knowledge store contributed each answer.

---

## 11. Security Considerations

### 11.1 Binding and Network Exposure

The K2K server binds to `127.0.0.1` by default. It is not reachable from other machines on the network unless explicitly reconfigured. This means:

- Admin endpoints (approve/reject clients) are inherently protected -- only local processes can reach them.
- The mDNS service advertises the port, but the server itself only accepts connections from localhost. For LAN federation, the operator must bind to `0.0.0.0` or a specific interface and configure appropriate firewall rules.

### 11.2 Client Allowlist

Two layers of access control:

1. **Registration gate.** Clients must register their public key and be approved before any authenticated request succeeds.
2. **Allowed clients list.** An optional `allowed_clients` config list further restricts which approved clients can access protected endpoints. If the list is non-empty, any client not in the list receives a 403 Forbidden.

### 11.3 Path Whitelisting

The `security.allowed_paths` configuration defines which filesystem paths are searchable. Every query filter's `paths` field is validated against this whitelist. If a requested path is not under any allowed path, the request is rejected with 403.

If `allowed_paths` is empty, all path-filtered queries are denied (fail-closed).

### 11.4 Store-Level ACL

Article CRUD operations enforce ownership:

- Personal store owners can read/write their own stores.
- Other clients need explicit grants in `k2k.client_store_grants`.
- Unauthorized read attempts return 404 (not 403) to avoid revealing resource existence.

### 11.5 Rate Limiting

Per-client rate limiting uses a sliding window algorithm:

- Default: 10 tasks per 60-second window (configurable).
- When exceeded, the server returns 429 with a `Retry-After` header.
- Stale client windows are cleaned up every 5 minutes.

Additionally, per-client storage quotas limit the number of articles a client can store.

### 11.6 Input Validation

- Query strings are capped at 10,000 characters.
- Article titles are capped at 1,000 characters.
- Article content is capped at 1,500,000 characters.
- `top_k` is capped at 100 for queries.
- Task timeouts are capped at 300 seconds by default.
- Remote handler error messages are sanitized (newlines stripped, truncated to 200 characters) to prevent log injection.

### 11.7 SSRF Protection

Remote capability handlers reject non-HTTPS endpoint URLs at both registration time and execution time. This prevents:

- Malicious mDNS announcements from directing traffic to internal services.
- Configuration errors that would expose internal endpoints.

### 11.8 Constant-Time Comparison

The registration secret comparison uses the `subtle` crate's constant-time equality check to prevent timing side-channel attacks.

### 11.9 JWT Replay Prevention

Each JWT includes a `jti` (JWT ID) claim with a UUIDv4 value. Combined with the 5-minute expiration window, this provides protection against token replay.

### 11.10 IDOR Prevention

Task status, cancellation, article access, conversation access, federation agreement listing, and SSE event streams all enforce ownership checks. A client can only see and modify resources that belong to them. Unauthorized access returns 404 rather than 403.

### 11.11 Task Persistence

Implementations SHOULD persist task state to durable storage so that in-progress tasks survive node restarts. The reference implementation uses SQLite. At minimum, the `task_id`, `status`, `result`, and timestamps (`created_at`, `updated_at`) MUST be recoverable after a restart. Implementations MAY also persist the `trace_id`, `capability_id`, and `input` fields to support post-restart diagnostics and retry logic.

---

## 12. Reference Implementation

The reference implementation is split across two Rust crates:

### 12.1 Crate Overview

### k2k-common

Shared library crate at `k2k-common/`. Contains:

- `models.rs` -- All protocol data structures (requests, responses, JWT claims, capabilities, tasks, SSE events, managed policy)
- `jwt.rs` -- RS256 JWT verification using the `jsonwebtoken` crate
- `client.rs` -- `K2KClient` for querying nodes, `KnManagementClient` for the central management API, and `generate_rsa_keypair()` utility

k2k-common is the canonical type library for all K2K protocol types. Server implementations SHOULD import types from k2k-common rather than defining their own copies. Enterprise management types (managed policies, heartbeat, instance registration) are NOT part of the core protocol and live in separate crates.

Add as a dependency:

```toml
[dependencies]
k2k-common = { path = "k2k-common" }
```

### K2K Server (knowledge-nexus-agent)

The server implementation lives in `src/k2k/`:

| File | Purpose |
|------|---------|
| `server.rs` | Axum HTTP server setup, route registration, CORS |
| `handlers.rs` | All HTTP endpoint handlers (query, articles, conversations, federation, tasks) |
| `middleware.rs` | JWT authentication middleware |
| `keys.rs` | RSA key management, client registration, JWT verification |
| `models.rs` | Server-side model definitions |
| `capabilities.rs` | Dynamic capability registry with built-in, config, and discovered sources |
| `tasks.rs` | Task queue with concurrency control, rate limiting, and broadcast events |
| `task_handlers.rs` | Built-in handlers for `semantic_search` and `web_search` |
| `remote_handler.rs` | HTTP proxy handler for remote capabilities |

Supporting modules:

| Module | Purpose |
|--------|---------|
| `src/discovery/` | mDNS advertisement and browsing, node registry |
| `src/federation/` | Federation agreements, remote query fan-out |
| `src/router/` | Query routing pipeline (classifier, planner, executor, merger) |
| `src/connection/` | WebSocket Hub connection with heartbeat |

### 12.2 Conformance Testing

Implementations claiming K2K v1.1 conformance SHOULD pass the conformance test suite. The k2k-node reference implementation includes integration tests covering:

- Client registration and key exchange
- JWT authentication (valid tokens, expired tokens, unknown clients)
- Query request/response cycle
- Task lifecycle (submission, status polling, completion, cancellation)
- Capability advertisement and manifest fetching
- `trace_id` propagation across query and task boundaries
- Protocol version negotiation (header handling, forward-compat unknown field ignoring)

The conformance tests are located in the reference implementation's test suite and can be run against any K2K v1.1-compatible server by configuring the target endpoint URL.

---

## Appendix A: Complete Endpoint Reference

### Public Endpoints (No Auth)

| Method | Path | Description |
|--------|------|-------------|
| GET | `/k2k/v1/health` | Health check |
| GET | `/k2k/v1/info` | Node identity and public key |
| POST | `/k2k/v1/register-client` | Register a new client |
| GET | `/k2k/v1/capabilities` | List capabilities (compact) |
| GET | `/k2k/v1/capabilities/manifest` | List capabilities (full metadata) |
| GET | `/k2k/v1/capabilities/refresh` | Hot-reload capabilities from config |
| GET | `/k2k/v1/discover-services` | Probe known services for manifests |
| GET | `/.well-known/k2k-manifest` | This node's capability manifest |

### Admin Endpoints (Localhost Only)

| Method | Path | Description |
|--------|------|-------------|
| GET | `/k2k/v1/admin/pending-clients` | List pending client registrations |
| POST | `/k2k/v1/admin/approve-client` | Approve a client |
| POST | `/k2k/v1/admin/reject-client` | Reject a client |

### Protected Endpoints (JWT Required)

| Method | Path | Description |
|--------|------|-------------|
| POST | `/k2k/v1/query` | Direct semantic search |
| POST | `/k2k/v1/route` | Smart routed query |
| GET | `/k2k/v1/stores` | List knowledge stores |
| GET | `/k2k/v1/nodes` | List discovered nodes |
| POST | `/k2k/v1/federate` | Create federation agreement |
| GET | `/k2k/v1/federations` | List federation agreements |
| POST | `/k2k/v1/tasks` | Submit a task |
| GET | `/k2k/v1/tasks/{id}` | Get task status |
| DELETE | `/k2k/v1/tasks/{id}` | Cancel a task |
| GET | `/k2k/v1/tasks/events` | SSE stream of task events |
| POST | `/api/v1/articles` | Create an article |
| GET | `/api/v1/articles/{id}` | Get an article |
| PUT | `/api/v1/articles/{id}` | Update an article |
| PATCH | `/api/v1/articles/{id}` | Partial update an article |
| DELETE | `/api/v1/articles/{id}` | Delete an article |
| GET | `/api/v1/stores/{id}/articles` | List articles in a store |
| POST | `/api/v1/conversations` | Create a conversation |
| GET | `/api/v1/conversations/{id}` | Get conversation with messages |
| POST | `/api/v1/conversations/{id}/messages` | Add a message |
| GET | `/api/v1/users/{id}/conversations` | List user conversations |
| POST | `/api/v1/conversations/{id}/extract` | Extract knowledge from conversation |
| POST | `/api/v1/web-clips` | Ingest a web clip |
| GET | `/api/v1/connectors` | List connectors |

---

## Appendix B: Configuration Keys

The K2K server behavior is controlled by these configuration keys:

| Key | Type | Description |
|-----|------|-------------|
| `k2k.port` | u16 | HTTP server port |
| `k2k.allowed_clients` | string[] | Client IDs permitted to access protected endpoints |
| `k2k.allowed_origins` | string[] | CORS allowed origins |
| `k2k.require_client_registration` | bool | Whether client registration is gated |
| `k2k.registration_secret` | string | Pre-shared key for auto-approval |
| `k2k.per_client_rate_limit` | u32 | Max tasks per rate window |
| `k2k.per_client_window_seconds` | u64 | Rate window duration |
| `k2k.per_client_article_quota` | usize | Max articles per client |
| `k2k.client_store_grants` | map | Per-client store access grants |
| `security.allowed_paths` | string[] | Filesystem paths allowed for search |
| `security.blocked_patterns` | string[] | Glob patterns to exclude from search |
| `discovery.browse_interval_secs` | u64 | mDNS browse cycle interval |
| `discovery.trusted_node_ids` | string[] | Node IDs allowed to federate |
| `discovery.bootstrap_nodes` | string[] | URLs of bootstrap nodes for WAN discovery |
| `device.id` | string | This node's UUID |
| `device.name` | string | Human-readable node name |

---

## Appendix C: JSON Schema Definitions

This appendix provides formal JSON Schema definitions for the core K2K message types.

### K2KQueryRequest

```json
{
  "$schema": "https://json-schema.org/draft/2020-12/schema",
  "title": "K2KQueryRequest",
  "type": "object",
  "properties": {
    "query": {
      "type": "string",
      "maxLength": 10000,
      "description": "Natural language search query"
    },
    "requesting_store": {
      "type": "string",
      "description": "ID of the requesting knowledge base"
    },
    "top_k": {
      "type": "integer",
      "default": 10,
      "minimum": 1,
      "maximum": 100,
      "description": "Maximum number of results"
    },
    "filters": {
      "type": ["object", "null"],
      "properties": {
        "paths": {
          "type": "array",
          "items": { "type": "string" }
        },
        "file_types": {
          "type": "array",
          "items": { "type": "string" }
        },
        "max_file_size_bytes": {
          "type": "integer"
        }
      },
      "description": "Path, file type, and size filters"
    },
    "context": {
      "type": ["string", "null"],
      "description": "Contextual hint for result weighting"
    },
    "target_stores": {
      "type": ["array", "null"],
      "items": { "type": "string" },
      "description": "Restrict search to specific stores"
    },
    "trace_id": {
      "type": ["string", "null"],
      "description": "Distributed tracing correlation ID"
    }
  },
  "required": ["query", "requesting_store"]
}
```

### K2KQueryResponse

```json
{
  "$schema": "https://json-schema.org/draft/2020-12/schema",
  "title": "K2KQueryResponse",
  "type": "object",
  "properties": {
    "query_id": {
      "type": "string",
      "description": "Unique identifier for this query"
    },
    "results": {
      "type": "array",
      "items": {
        "type": "object",
        "properties": {
          "article_id": { "type": "string" },
          "store_id": { "type": "string" },
          "title": { "type": "string" },
          "summary": { "type": "string" },
          "content": { "type": "string" },
          "confidence": { "type": "number", "minimum": 0, "maximum": 1 },
          "source_type": { "type": "string" },
          "tags": { "type": "array", "items": { "type": "string" } },
          "metadata": { "type": "object" },
          "provenance": {
            "type": "object",
            "properties": {
              "store_id": { "type": "string" },
              "store_type": { "type": "string" },
              "original_rank": { "type": "integer" },
              "rrf_score": { "type": "number" }
            }
          }
        }
      },
      "description": "Ranked search results"
    },
    "total_results": {
      "type": "integer",
      "description": "Total number of results returned"
    },
    "stores_queried": {
      "type": "array",
      "items": { "type": "string" },
      "description": "IDs of stores that were searched"
    },
    "query_time_ms": {
      "type": "integer",
      "description": "Query execution time in milliseconds"
    },
    "routing_decision": {
      "type": ["object", "null"],
      "properties": {
        "scope": { "type": "string" },
        "stores_selected": { "type": "integer" },
        "federation_enabled": { "type": "boolean" }
      },
      "description": "Routing metadata (present for routed queries)"
    },
    "trace_id": {
      "type": ["string", "null"],
      "description": "Echoed trace ID from the request"
    }
  },
  "required": ["query_id", "results", "total_results", "stores_queried", "query_time_ms"]
}
```

### TaskRequest

```json
{
  "$schema": "https://json-schema.org/draft/2020-12/schema",
  "title": "TaskRequest",
  "type": "object",
  "properties": {
    "capability_id": {
      "type": "string",
      "description": "The capability to invoke"
    },
    "input": {
      "type": "object",
      "description": "Input payload for the capability handler"
    },
    "requesting_node_id": {
      "type": ["string", "null"],
      "description": "The originating node's ID"
    },
    "timeout_seconds": {
      "type": "integer",
      "default": 60,
      "maximum": 300,
      "description": "Maximum execution time in seconds"
    },
    "context": {
      "type": ["string", "null"],
      "description": "Contextual hint for task execution"
    },
    "priority": {
      "type": "string",
      "enum": ["low", "normal", "high"],
      "default": "normal",
      "description": "Task priority"
    },
    "trace_id": {
      "type": ["string", "null"],
      "description": "Distributed tracing correlation ID"
    }
  },
  "required": ["capability_id", "input"]
}
```

### TaskStatusResponse

```json
{
  "$schema": "https://json-schema.org/draft/2020-12/schema",
  "title": "TaskStatusResponse",
  "type": "object",
  "properties": {
    "task_id": {
      "type": "string",
      "description": "Unique task identifier"
    },
    "status": {
      "type": "string",
      "enum": ["queued", "running", "completed", "failed", "cancelled"],
      "description": "Current task status"
    },
    "result": {
      "type": ["object", "null"],
      "properties": {
        "data": { "type": "object" },
        "duration_ms": { "type": "integer" }
      },
      "description": "Task result (present when completed)"
    },
    "error": {
      "type": ["string", "null"],
      "description": "Error message (present when failed)"
    },
    "created_at": {
      "type": "string",
      "format": "date-time",
      "description": "Task creation timestamp"
    },
    "updated_at": {
      "type": "string",
      "format": "date-time",
      "description": "Last update timestamp"
    },
    "progress": {
      "type": "integer",
      "minimum": 0,
      "maximum": 100,
      "description": "Task progress percentage"
    },
    "trace_id": {
      "type": ["string", "null"],
      "description": "Echoed trace ID from the submission"
    }
  },
  "required": ["task_id", "status", "created_at", "updated_at"]
}
```

### RegisterClientRequest

```json
{
  "$schema": "https://json-schema.org/draft/2020-12/schema",
  "title": "RegisterClientRequest",
  "type": "object",
  "properties": {
    "client_id": {
      "type": "string",
      "description": "Unique client identifier"
    },
    "client_name": {
      "type": "string",
      "description": "Human-readable client name"
    },
    "public_key_pem": {
      "type": "string",
      "description": "RSA public key in PEM (SPKI) format"
    },
    "registration_secret": {
      "type": ["string", "null"],
      "description": "Optional pre-shared key for auto-approval"
    }
  },
  "required": ["client_id", "client_name", "public_key_pem"]
}
```

### HealthResponse

```json
{
  "$schema": "https://json-schema.org/draft/2020-12/schema",
  "title": "HealthResponse",
  "type": "object",
  "properties": {
    "status": {
      "type": "string",
      "enum": ["healthy", "degraded", "unhealthy"],
      "description": "Node health status"
    },
    "node_id": {
      "type": "string",
      "description": "This node's UUID"
    },
    "node_type": {
      "type": "string",
      "description": "Node type (e.g., system_agent)"
    },
    "capabilities": {
      "type": "array",
      "items": { "type": "string" },
      "description": "List of capability IDs"
    },
    "indexed_files": {
      "type": "integer",
      "description": "Number of indexed files"
    },
    "uptime_seconds": {
      "type": "integer",
      "description": "Node uptime in seconds"
    }
  },
  "required": ["status", "node_id", "node_type", "capabilities"]
}
```

### AgentCapability

```json
{
  "$schema": "https://json-schema.org/draft/2020-12/schema",
  "title": "AgentCapability",
  "type": "object",
  "properties": {
    "id": {
      "type": "string",
      "description": "Unique capability identifier"
    },
    "name": {
      "type": "string",
      "description": "Human-readable capability name"
    },
    "category": {
      "type": "string",
      "enum": ["Knowledge", "Tool", "Skill", "Compute"],
      "description": "Capability category"
    },
    "description": {
      "type": "string",
      "description": "What this capability does"
    },
    "min_protocol_version": {
      "type": "string",
      "default": "1.0",
      "description": "Minimum K2K protocol version required to invoke this capability"
    },
    "input_schema": {
      "type": "object",
      "description": "JSON Schema for the capability's input"
    },
    "output_schema": {
      "type": ["object", "null"],
      "description": "JSON Schema for the capability's output"
    },
    "endpoint": {
      "type": "string",
      "description": "URL path for invoking the capability"
    },
    "method": {
      "type": "string",
      "enum": ["GET", "POST", "PUT", "PATCH"],
      "description": "HTTP method for invocation"
    },
    "supports_streaming": {
      "type": "boolean",
      "default": false,
      "description": "Whether this capability supports SSE streaming"
    }
  },
  "required": ["id", "name", "category", "description", "input_schema", "endpoint", "method"]
}
```
