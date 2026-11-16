#!/usr/bin/env bash
# SPDX-License-Identifier: MIT OR Apache-2.0
# SurfaceAnt relay deploy (ROADMAP row 4). Runbook and prerequisites: docs/DEPLOY.md.
#
#   deploy/deploy.sh check     build + `wrangler deploy --dry-run`; needs no credentials, changes nothing
#   deploy/deploy.sh secrets   set CHANNEL_KEY and ADMIN_SECRET from the environment (values via stdin, never argv)
#   deploy/deploy.sh deploy    build, deploy with the thinkersjournal.net routes; needs DEPLOY_GO=1
#   deploy/deploy.sh smoke     post-deploy smoke test against https://thinkersjournal.net (needs ADMIN_SECRET)
#
# Environment: CLOUDFLARE_API_TOKEN (and CLOUDFLARE_ACCOUNT_ID if the token sees several accounts), supplied by the
# caller (the PM, under with-secret). Nothing here reads, prints or stores a secret value.
set -euo pipefail
cd "$(dirname "$0")/.."
export RUSTC_WRAPPER=   # sccache breaks the wasm build
HOST="${SURFACEANT_HOST:-thinkersjournal.net}"
ROUTES=(--route "$HOST/hooks/*" --route "$HOST/connect/*" --route "$HOST/admin/*")

need() { [ -n "${!1:-}" ] || { echo "missing env $1" >&2; exit 2; }; }

build() {
  (cd worker && { [ -d node_modules ] || npm ci; } && worker-build --release)
}

case "${1:-}" in
  check)
    build
    (cd worker && npx wrangler deploy --dry-run "${ROUTES[@]}")
    ;;
  secrets)
    need CLOUDFLARE_API_TOKEN; need CHANNEL_KEY; need ADMIN_SECRET
    # The Worker must exist before a secret can be put on it, so `deploy` runs first; `secret put` creates a new version.
    printf '%s' "$CHANNEL_KEY"  | (cd worker && npx wrangler secret put CHANNEL_KEY)
    printf '%s' "$ADMIN_SECRET" | (cd worker && npx wrangler secret put ADMIN_SECRET)
    ;;
  deploy)
    need CLOUDFLARE_API_TOKEN
    [ "${DEPLOY_GO:-}" = 1 ] || { echo "refusing: set DEPLOY_GO=1 only after the PM's go" >&2; exit 2; }
    build
    (cd worker && npx wrangler deploy "${ROUTES[@]}")
    ;;
  smoke)
    need ADMIN_SECRET
    SMOKE_RELAY="https://$HOST" SMOKE_ADMIN_SECRET="$ADMIN_SECRET" cargo run --release -q -p surfaceant-client --example smoke
    ;;
  *)
    sed -n '2,12p' "$0"; exit 2 ;;
esac
