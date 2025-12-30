# sentry-cli

[![CI](https://github.com/Osso/sentry-cli/actions/workflows/ci.yml/badge.svg)](https://github.com/Osso/sentry-cli/actions/workflows/ci.yml)
[![GitHub release](https://img.shields.io/github/v/release/Osso/sentry-cli)](https://github.com/Osso/sentry-cli/releases)
[![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg)](LICENSE)

CLI for Sentry API access.

## Installation

```bash
cargo install --path .
```

## Setup

```bash
sentry config
```

## Usage

```bash
sentry projects         # List projects
sentry issues           # List issues for a project
sentry issue <id>       # Get issue details
```

## License

MIT
