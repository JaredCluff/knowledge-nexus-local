#!/usr/bin/env python3
"""
Generate HS256 JWT tokens for System Agent authentication with Agent Hub.

This script generates JWT tokens that agents can use to authenticate
with the Agent Hub WebSocket endpoint.
"""

import argparse
import jwt
import json
from datetime import datetime, timedelta
from pathlib import Path
import sys

# Replace with your actual JWT secret. Never commit real secrets.
DEFAULT_JWT_SECRET = "REPLACE_WITH_YOUR_JWT_SECRET"

def generate_agent_token(
    user_id: str,
    device_id: str,
    jwt_secret: str = DEFAULT_JWT_SECRET,
    expiration_days: int = 365
) -> str:
    """
    Generate a JWT token for agent authentication.

    Args:
        user_id: User ID that owns this agent
        device_id: Unique device identifier
        jwt_secret: JWT secret key (must match agent-hub)
        expiration_days: Token validity period in days

    Returns:
        JWT token string
    """
    now = datetime.now(datetime.UTC) if hasattr(datetime, 'UTC') else datetime.utcnow()
    exp = now + timedelta(days=expiration_days)

    payload = {
        "sub": user_id,  # Subject (user ID)
        "device_id": device_id,
        "iat": int(now.timestamp()),  # Issued at
        "exp": int(exp.timestamp()),  # Expiration
        "type": "agent",
        "iss": "knowledge-nexus"  # Issuer
    }

    token = jwt.encode(payload, jwt_secret, algorithm="HS256")

    return token


def update_agent_config(token: str, config_path: str = None) -> None:
    """
    Update agent config file with new token.

    Args:
        token: JWT token to set
        config_path: Path to config.yaml (auto-detected if not provided)
    """
    if not config_path:
        # Try to find config in standard locations
        possible_paths = [
            Path.home() / "Library/Application Support/knowledge-nexus-agent/config.yaml",
            Path.home() / ".config/knowledge-nexus-agent/config.yaml",
            Path("config.yaml"),
        ]

        for path in possible_paths:
            if path.exists():
                config_path = str(path)
                break

        if not config_path:
            print("Could not find config.yaml. Please specify path with --config")
            return

    config_path = Path(config_path)
    if not config_path.exists():
        print(f"Config file not found: {config_path}")
        return

    # Read config
    with open(config_path, 'r') as f:
        content = f.read()

    # Replace auth_token line
    import re
    updated = re.sub(
        r'auth_token:.*',
        f'auth_token: "{token}"',
        content
    )

    # Write back
    with open(config_path, 'w') as f:
        f.write(updated)

    print(f"✓ Updated config at: {config_path}")


def main():
    parser = argparse.ArgumentParser(
        description="Generate JWT tokens for System Agent authentication"
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
        "--jwt-secret",
        default=DEFAULT_JWT_SECRET,
        help="JWT secret key (must match agent-hub backend)"
    )
    parser.add_argument(
        "--expiration-days",
        type=int,
        default=365,
        help="Token validity period in days (default: 365)"
    )
    parser.add_argument(
        "--config",
        help="Path to agent config.yaml file"
    )
    parser.add_argument(
        "--update-config",
        action="store_true",
        help="Automatically update agent config with new token"
    )
    parser.add_argument(
        "--output-json",
        action="store_true",
        help="Output token info as JSON"
    )

    args = parser.parse_args()

    # Get device_id from config if not provided
    device_id = args.device_id
    if not device_id:
        # Try to read from config
        config_path = args.config
        if not config_path:
            possible_paths = [
                Path.home() / "Library/Application Support/knowledge-nexus-agent/config.yaml",
                Path.home() / ".config/knowledge-nexus-agent/config.yaml",
            ]
            for path in possible_paths:
                if path.exists():
                    config_path = str(path)
                    break

        if config_path and Path(config_path).exists():
            import yaml
            with open(config_path, 'r') as f:
                config = yaml.safe_load(f)
                device_id = config.get('device', {}).get('id')

        if not device_id:
            print("Error: --device-id required (could not auto-detect from config)")
            sys.exit(1)

    # Generate token
    token = generate_agent_token(
        user_id=args.user_id,
        device_id=device_id,
        jwt_secret=args.jwt_secret,
        expiration_days=args.expiration_days
    )

    # Decode to show claims (for verification)
    # Use leeway to avoid clock skew issues
    claims = jwt.decode(token, args.jwt_secret, algorithms=["HS256"], options={"verify_iat": False})

    if args.output_json:
        output = {
            "token": token,
            "claims": claims,
            "user_id": args.user_id,
            "device_id": device_id,
            "expires_at": datetime.fromtimestamp(claims['exp']).isoformat()
        }
        print(json.dumps(output, indent=2))
    else:
        print("="*60)
        print("Agent JWT Token Generated")
        print("="*60)
        print(f"\nUser ID: {args.user_id}")
        print(f"Device ID: {device_id}")
        print(f"Expires: {datetime.fromtimestamp(claims['exp']).isoformat()}")
        print(f"\nToken:")
        print(token)
        print()

    # Update config if requested
    if args.update_config:
        update_agent_config(token, args.config)
        print("\n✓ Config updated - restart agent for changes to take effect")


if __name__ == "__main__":
    main()
