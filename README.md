# Knowledge Nexus Local Agent

A standalone CLI agent for K2K (Knowledge-to-Knowledge) federation, local file indexing, and semantic search. Runs headless as a background service or with an optional system tray icon. Designed to operate fully offline with no cloud dependencies for core functionality.

## Features

- **K2K Federation Protocol** - Connect and share knowledge across nodes using the K2K v1.1 protocol
- **Local File Indexing** - Recursive file walker with configurable watch directories and real-time change detection
- **ONNX Embedding Generation** - Local embedding inference using all-MiniLM-L6-v2, fully offline
- **LanceDB Semantic Search** - Embedded vector database for fast similarity search over indexed documents
- **mDNS/DNS-SD Discovery** - Automatic peer discovery on the local network
- **RSA-256 JWT Authentication** - Secure agent-to-hub authentication
- **System Tray Icon** - Optional system tray integration on macOS and Linux

## Quick Start

### Build from Source

```bash
git clone https://github.com/jaredcluff/knowledge-nexus-local.git
cd knowledge-nexus-local
cargo build --release
```

The binary will be at `target/release/knowledge-nexus-agent`.

### Run

```bash
# Start the agent
./target/release/knowledge-nexus-agent start

# Start without system tray
./target/release/knowledge-nexus-agent start --no-tray

# Run with debug logging
RUST_LOG=knowledge_nexus_agent=debug ./target/release/knowledge-nexus-agent start
```

## Configuration

Configuration follows XDG conventions:

| Platform | Config Path |
|----------|-------------|
| macOS    | `~/Library/Application Support/knowledge-nexus-agent/config.yaml` |
| Linux    | `~/.config/knowledge-nexus-agent/config.yaml` |
| Windows  | `%APPDATA%\knowledge-nexus-agent\config.yaml` |

### Environment Variables

| Variable | Description |
|----------|-------------|
| `K2K_HUB_URL` | WebSocket URL of the Agent Hub |
| `K2K_AUTH_TOKEN` | JWT authentication token |
| `K2K_DEVICE_ID` | Override device ID |
| `RUST_LOG` | Logging filter (e.g., `knowledge_nexus_agent=debug`) |

## K2K Protocol

See the [K2K Protocol Specification](docs/K2K_PROTOCOL.md) for details on the federation protocol.

## License

Apache 2.0 - see [LICENSE](LICENSE) for details.
