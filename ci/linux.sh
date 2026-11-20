#!/bin/sh
set -eu
uid=$(id -u)
setup='git config --global user.email ci@example.com; git config --global user.name ci; git config --global init.defaultBranch main'
echo "== GNU"
docker run --rm -v "$PWD":/src -w /src/remote golang:1.27 sh -ec "
  apt-get update -qq >/dev/null && apt-get install -y -qq tmux fish tcsh git-lfs shellcheck >/dev/null
  useradd -m -u $uid ci
  su ci -c 'export PATH=/usr/local/go/bin:\$PATH GOFLAGS=-buildvcs=false GOTOOLCHAIN=auto; $setup; shellcheck --severity=style scripts/*.sh internal/*/scripts/*.sh ../ci/*.sh ../plugin/bin/magi && go vet ./... && go test -count=1 ./...'
"
echo "== BusyBox"
docker run --rm -v "$PWD":/src -w /src/remote alpine:3.20 sh -ec "
  apk add --no-cache git git-lfs tmux go curl >/dev/null
  adduser -D -u $uid ci
  su ci -c 'export GOFLAGS=-buildvcs=false GOTOOLCHAIN=auto; $setup; go test -count=1 ./...'
"
