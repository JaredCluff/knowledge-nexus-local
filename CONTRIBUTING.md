# Contributing to Knowledge Nexus Local Agent

Thank you for your interest in contributing! This document provides guidelines for contributing to the project.

## Reporting Issues

Please report bugs and request features via [GitHub Issues](https://github.com/jaredcluff/knowledge-nexus-local/issues).

When reporting a bug, include:
- Operating system and version
- Rust toolchain version (`rustc --version`)
- Steps to reproduce
- Expected vs actual behavior
- Relevant log output

## Development Setup

### Prerequisites

- Rust stable toolchain (install via [rustup](https://rustup.rs/))
- On Linux: `libgtk-3-dev`, `libayatana-appindicator3-dev` (for system tray support)

### Build and Test

```bash
# Build
cargo build

# Run tests
cargo test

# Run linter
cargo clippy

# Format code
cargo fmt
```

## Pull Request Process

1. Fork the repository
2. Create a feature branch from `main`
3. Make your changes
4. Ensure all tests pass (`cargo test`)
5. Ensure code is formatted (`cargo fmt`) and lint-clean (`cargo clippy`)
6. Submit a pull request against `main`

## Code Style

- Run `cargo fmt` before committing
- All code must pass `cargo clippy` without warnings
- Write tests for new functionality
- Keep commits focused and atomic
