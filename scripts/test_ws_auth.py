#!/usr/bin/env python3
"""Test WebSocket authentication with the generated JWT token."""

import asyncio
import websockets
import json
import yaml
from pathlib import Path

async def test_auth():
    # Load config to get auth token
    config_paths = [
        Path.home() / "Library/Application Support/knowledge-nexus-agent/config.yaml",
        Path.home() / ".config/knowledge-nexus-agent/config.yaml",
    ]

    config_path = None
    for path in config_paths:
        if path.exists():
            config_path = path
            break

    if not config_path:
        print("Error: Could not find config.yaml")
        return

    with open(config_path, 'r') as f:
        config = yaml.safe_load(f)

    auth_token = config.get('connection', {}).get('auth_token')
    device_id = config.get('device', {}).get('id')
    device_name = config.get('device', {}).get('name')
    hub_url = config.get('connection', {}).get('hub_url', 'wss://localhost:8765/ws/hub')

    if not auth_token:
        print("Error: No auth_token in config")
        return

    print("="*60)
    print("Testing WebSocket Authentication")
    print("="*60)
    print(f"Hub URL: {hub_url}")
    print(f"Device ID: {device_id}")
    print(f"Device Name: {device_name}")
    print(f"Token (first 50 chars): {auth_token[:50]}...")
    print()

    try:
        print("Connecting to WebSocket...")
        async with websockets.connect(hub_url, ping_interval=None) as ws:
            print("✓ Connected!")

            # Send auth request
            auth_request = {
                "type": "auth_request",
                "payload": {
                    "protocol_version": "1.0",
                    "auth_method": "jwt",
                    "token": auth_token,
                    "device": {
                        "device_id": device_id,
                        "device_name": device_name,
                        "platform": "darwin",
                        "platform_version": "unknown",
                        "agent_version": "0.8.0"
                    },
                    "capabilities": ["query", "read_file", "list_dir"],
                    "allowed_paths": ["/home/user/Documents", "/home/user/Projects"],
                    "blocked_patterns": []
                }
            }

            print(f"\nSending auth_request...")
            await ws.send(json.dumps(auth_request))

            # Wait for response
            print("Waiting for auth_response...")
            response_raw = await asyncio.wait_for(ws.recv(), timeout=10.0)
            response = json.loads(response_raw)

            print(f"\n✓ Received response:")
            print(json.dumps(response, indent=2))

            if response.get("type") == "auth_response":
                payload = response.get("payload", {})
                if payload.get("success"):
                    print(f"\n✅ Authentication SUCCESSFUL!")
                    print(f"   Session ID: {payload.get('session_id')}")
                    print(f"   User ID: {payload.get('user_id')}")
                    print(f"   Heartbeat interval: {payload.get('heartbeat_interval_seconds')}s")
                else:
                    print(f"\n❌ Authentication FAILED!")
                    print(f"   Error code: {payload.get('error_code')}")
                    print(f"   Error message: {payload.get('error_message')}")
            else:
                print(f"\n⚠️  Unexpected response type: {response.get('type')}")

    except asyncio.TimeoutError:
        print("\n❌ Timeout waiting for auth response")
    except Exception as e:
        print(f"\n❌ Error: {e}")

if __name__ == "__main__":
    asyncio.run(test_auth())
