#!/usr/bin/env python3
"""Decode and inspect a JWT token without verification."""

import jwt
import json
import sys

if len(sys.argv) < 2:
    print("Usage: python3 decode_token.py <token>")
    sys.exit(1)

token = sys.argv[1]

try:
    # Decode without verification to see claims
    decoded = jwt.decode(token, options={"verify_signature": False})

    print("="*60)
    print("JWT Token Claims (decoded without verification)")
    print("="*60)
    print(json.dumps(decoded, indent=2))

    # Try to decode header
    header = jwt.get_unverified_header(token)
    print("\n" + "="*60)
    print("JWT Header")
    print("="*60)
    print(json.dumps(header, indent=2))

except Exception as e:
    print(f"Error decoding token: {e}")
