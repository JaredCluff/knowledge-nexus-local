#!/usr/bin/env python3
"""
Request an agent token from the production Agent Hub.

This script calls the /generate_token endpoint to get a properly signed
JWT token that the agent can use for authentication.
"""

import argparse
import requests
import json
import yaml
from pathlib import Path
import sys
import re


def load_config():
    """Load agent config to get device ID."""
    config_paths = [
        Path.home() / "Library/Application Support/knowledge-nexus-agent/config.yaml",
        Path.home() / ".config/knowledge-nexus-agent/config.yaml",
    ]

    for path in config_paths:
        if path.exists():
            with open(path, 'r') as f:
                return yaml.safe_load(f), path

    return None, None


def update_config(token: str, config_path: Path):
    """Update agent config with new token."""
    with open(config_path, 'r') as f:
        content = f.read()

    # Replace auth_token line
    updated = re.sub(
        r'auth_token:.*',
        f'auth_token: "{token}"',
        content
    )

    with open(config_path, 'w') as f:
        f.write(updated)

    print(f"✓ Updated config at: {config_path}")


def request_token(
    hub_url: str,
    device_id: str,
    user_id: str,
    k2k_auth_token: str,
    expiration_days: int = 365
):
    """Request an agent token from the hub."""

    endpoint = f"{hub_url.replace('/ws', '')}/generate_token"

    print("="*60)
    print("Requesting Agent Token")
    print("="*60)
    print(f"Endpoint: {endpoint}")
    print(f"Device ID: {device_id}")
    print(f"User ID: {user_id}")
    print()

    try:
        response = requests.post(
            endpoint,
            params={
                "device_id": device_id,
                "user_id": user_id,
                "expiration_days": expiration_days
            },
            headers={
                "Authorization": f"Bearer {k2k_auth_token}",
                "Content-Type": "application/json"
            },
            timeout=10
        )

        if response.status_code == 200:
            data = response.json()
            print("✅ Token generated successfully!")
            print()
            print("Token Info:")
            print(f"  User ID: {data.get('user_id')}")
            print(f"  Device ID: {data.get('device_id')}")
            print(f"  Expires: {data.get('expires_at')}")
            print(f"  Validity: {data.get('expires_in_days')} days")
            print()
            print("Token:")
            print(data.get('token'))
            print()

            return data.get('token')

        else:
            print(f"❌ Error: {response.status_code}")
            print(response.text)
            return None

    except Exception as e:
        print(f"❌ Request failed: {e}")
        return None


def main():
    parser = argparse.ArgumentParser(
        description="Request agent authentication token from hub"
    )
    parser.add_argument(
        "--hub-url",
        default="http://localhost:8765",
        help="Agent Hub base URL (default: http://localhost:8765)"
    )
    parser.add_argument(
        "--user-id",
        required=True,
        help="User ID that owns this agent"
    )
    parser.add_argument(
        "--device-id",
        help="Device ID (auto-detected from config if not provided)"
    )
    parser.add_argument(
        "--k2k-auth",
        help="K2K authentication token for the request (required for production)"
    )
    parser.add_argument(
        "--expiration-days",
        type=int,
        default=365,
        help="Token validity period in days (default: 365)"
    )
    parser.add_argument(
        "--update-config",
        action="store_true",
        help="Automatically update agent config with new token"
    )

    args = parser.parse_args()

    # Load config to get device ID if not provided
    config, config_path = load_config()
    device_id = args.device_id

    if not device_id and config:
        device_id = config.get('device', {}).get('id')

    if not device_id:
        print("Error: --device-id required (could not auto-detect from config)")
        sys.exit(1)

    if not args.k2k_auth:
        print("Error: --k2k-auth required for production endpoints")
        print("Provide a valid K2K authentication token")
        sys.exit(1)

    # Request token
    token = request_token(
        hub_url=args.hub_url,
        device_id=device_id,
        user_id=args.user_id,
        k2k_auth_token=args.k2k_auth,
        expiration_days=args.expiration_days
    )

    if token and args.update_config and config_path:
        update_config(token, config_path)
        print("\n✓ Config updated - restart agent for changes to take effect")


if __name__ == "__main__":
    main()
