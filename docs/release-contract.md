# Release Contract

This repository publishes versioned `evelin` binaries from GitHub Actions to S3.

## Triggers

- `push` tags matching `v*`
- manual `workflow_dispatch` with a `version` input

## Version Rules

- The resolved workflow version must match `Cargo.toml`.
- Manual input may be passed as `0.1.0` or `v0.1.0`.
- A version mismatch fails the workflow before any upload happens.

## Artifact Naming

Each build job packages one host-specific release archive:

- Unix hosts: `evelin-v<version>-<host-target>.tar.gz`
- Windows hosts: `evelin-v<version>-<host-target>.zip`
- Checksum manifest: `SHA256SUMS`

Examples:

- `evelin-v0.1.0-x86_64-unknown-linux-gnu.tar.gz`
- `evelin-v0.1.0-aarch64-apple-darwin.tar.gz`
- `evelin-v0.1.0-x86_64-pc-windows-msvc.zip`

## S3 Layout

Required repository variables:

- `AWS_RELEASE_BUCKET`
- `AWS_RELEASE_ROLE_ARN`

Optional repository variables:

- `AWS_RELEASE_PREFIX` default `evelin`
- `AWS_RELEASE_REGION` default `eu-west-1`

Published object layout:

- `s3://<bucket>/<prefix>/v<version>/<artifact>`

Examples:

- `s3://my-release-bucket/evelin/v0.1.0/evelin-v0.1.0-x86_64-unknown-linux-gnu.tar.gz`
- `s3://my-release-bucket/evelin/v0.1.0/SHA256SUMS`

## Auth Model

- GitHub Actions requests an OIDC token with `id-token: write`.
- AWS auth is performed through `aws-actions/configure-aws-credentials`.
- The IAM role should restrict the trusted repository/ref and only allow writes to the chosen bucket/prefix.

## Failure Model

The workflow fails if:

- version resolution is empty or mismatched
- the required AWS repository variables are missing
- a runner fails to build or package its binary
- checksum generation fails
- S3 upload fails for any artifact
