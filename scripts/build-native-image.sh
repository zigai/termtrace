#!/usr/bin/env bash
set -euo pipefail

if [[ $# -ne 2 ]]; then
    echo "usage: $0 IMAGE_TAG BASE_IMAGE@sha256:DIGEST" >&2
    exit 2
fi

image=$1
base=$2
if [[ ! "$base" =~ @sha256:[0-9a-f]{64}$ ]]; then
    echo "BASE_IMAGE must be pinned by an exact sha256 digest" >&2
    exit 2
fi

version=$(python3 -c 'import tomllib; print(tomllib.load(open("Cargo.toml", "rb"))["package"]["version"])')
revision=$(git rev-parse HEAD)
docker build \
    --file packaging/native/Containerfile \
    --build-arg "BASE_IMAGE=$base" \
    --build-arg "BASE_IMAGE_DIGEST=${base##*@}" \
    --build-arg "SOURCE_REVISION=$revision" \
    --build-arg "TERMTRACE_VERSION=$version" \
    --tag "$image" \
    .

docker run --rm "$image" doctor --require semantic,native-screenshot,presentation
