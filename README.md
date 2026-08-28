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
sentry projects             # List projects
sentry issues <project>     # List issue aggregates for a project
sentry issue <id>           # Get issue details
sentry performance <project> # Rank slow transactions
```

### Project event search

Search project error events through Sentry Explore:

```text
sentry events <project> [--query <QUERY>] --start <RFC3339-UTC> --end <RFC3339-UTC> [--limit <1..=100>]
```

`--start` and `--end` are required UTC timestamps using `Z` or `+00:00`; `--start` must precede `--end`. `--limit` defaults to `100` and accepts `1` through `100`.

The command prints Explore's structured JSON response:

```json
{
  "data": [
    {
      "id": "…",
      "timestamp": "…",
      "title": "…",
      "issue": "…",
      "user.id": "…",
      "message": "…"
    }
  ],
  "meta": { "dataset": "errors" }
}
```

Example:

```bash
sentry events flutter --query 'user.id:762159' --start 2026-08-12T16:27:00Z --end 2026-08-12T16:29:00Z
```

`issue <id> events` remains the issue-scoped event listing; top-level `events <project>` searches project events by time range and query.

### Performance rankings

Rank project transactions by duration through Sentry Explore:

```text
sentry performance <project> [OPTIONS]
```

Defaults are environment `production`, period `24h`, metric `p95`, and limit `20`. Supported metrics are `avg`, `p75`, `p95`, and `p99`; durations are reported in milliseconds.

Options:

- `--environment <ENVIRONMENT>` — Sentry environment.
- `--period <PERIOD>` — relative period such as `24h`, `7d`, or `30d`.
- `--query <QUERY>` — additional Explore query appended to the environment and transaction filter.
- `--metric <avg|p75|p95|p99>` — duration aggregate used for ranking.
- `--limit <1..=100>` — maximum transactions returned.
- `--json` — print normalized JSON instead of the table.

Example page-route ranking:

```bash
sentry performance web --query 'transaction:www/*' --metric p95 --limit 20
```

The default output is a tab-separated table sorted slowest-first. It includes rank, full transaction name, sample count, and the selected duration metric:

```text
RANK    TRANSACTION    COUNT    P95 (MS)
1       www/a/:artist  42       1234.50
```

`--json` emits this stable shape, with rows in the same deterministic order:

```json
{
  "project": "web",
  "environment": "production",
  "period": "24h",
  "query": "environment:production is_transaction:true transaction:www/*",
  "metric": "p95",
  "rows": [
    {
      "transaction": "www/a/:artist",
      "count": 42,
      "duration_ms": 1234.5
    }
  ]
}
```

## License

MIT
