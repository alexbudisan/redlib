#!/usr/bin/env bash
# Build an arm64 Redlib image for the Pi and write, into dist/:
#   redlib-arm64-<id>.tar.gz           import in Portainer (Images -> Import) or `docker load -i`
#   portainer-stack-arm64-<id>.yaml    paste into the existing stack's editor and "Update the stack"
# Only the newest KEEP builds are kept in dist/ and in the local Docker image list.
set -euo pipefail

KEEP="${KEEP:-3}"
cd "$(dirname "$0")/.."

ID="$(date +%Y%m%d-%H%M%S)"
TAG="redlib-local:${ID}-arm64"
mkdir -p dist

docker info >/dev/null 2>&1 || { echo "Docker is not running; start Docker Desktop first." >&2; exit 1; }

echo "==> Building ${TAG}"
docker build --platform linux/arm64 -f Dockerfile.ubuntu --build-arg "REDLIB_BUILD_ID=${ID}" -t "${TAG}" .

echo "==> Exporting dist/redlib-arm64-${ID}.tar.gz"
docker save "${TAG}" | gzip >"dist/redlib-arm64-${ID}.tar.gz"

cat >"dist/portainer-stack-arm64-${ID}.yaml" <<EOF
services:
  redlib:
    image: ${TAG}
    container_name: redlib
    restart: unless-stopped
    init: true
    environment:
      - MALLOC_ARENA_MAX=2
      - RUST_LOG=redlib=info,warn
      - RUST_BACKTRACE=1
    ports:
      - "8666:8080"
    mem_limit: 512m
    memswap_limit: 512m
    cpus: 1.0
    pids_limit: 128
    logging:
      driver: json-file
      options:
        max-size: "10m"
        max-file: "3"
    read_only: true
    security_opt:
      - no-new-privileges:true
    cap_drop:
      - ALL
    healthcheck:
      test: ["CMD", "wget", "--spider", "-q", "--tries=1", "http://localhost:8080/settings"]
      interval: 1m
      timeout: 3s
      start_period: 10s
      retries: 3
EOF

echo "==> Keeping the newest ${KEEP} builds"
# Build IDs sort chronologically, so everything after the first KEEP is older.
ls -1 dist/redlib-arm64-*.tar.gz 2>/dev/null | sed -E 's/.*redlib-arm64-(.*)\.tar\.gz/\1/' | sort -r | tail -n +"$((KEEP + 1))" |
	while read -r old; do
		rm -f "dist/redlib-arm64-${old}.tar.gz" "dist/portainer-stack-arm64-${old}.yaml"
		docker image rm "redlib-local:${old}-arm64" >/dev/null 2>&1 || true
	done
# Stack files whose tarball is already gone.
for stack in dist/portainer-stack-arm64-*.yaml; do
	[ -e "$stack" ] || continue
	old="${stack#dist/portainer-stack-arm64-}"
	old="${old%.yaml}"
	[ -e "dist/redlib-arm64-${old}.tar.gz" ] || rm -f "$stack"
done

echo
echo "Done:"
ls -lh "dist/redlib-arm64-${ID}.tar.gz" "dist/portainer-stack-arm64-${ID}.yaml"
