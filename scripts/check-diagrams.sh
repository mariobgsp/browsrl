#!/usr/bin/env bash
set -euo pipefail

root_dir=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)
committed_dir="$root_dir/diagrams"
temporary_dir=$(mktemp -d "${TMPDIR:-/tmp}/rbrowse-diagrams.XXXXXX")
trap 'rm -rf "$temporary_dir"' EXIT

DIAGRAM_OUTPUT_DIR="$temporary_dir" \
PLANTUML_JAVA="${PLANTUML_JAVA:-java}" \
PLANTUML_JAR="${PLANTUML_JAR:-}" \
    "$root_dir/scripts/render-diagrams.sh" >/dev/null

shopt -s nullglob
sources=("$committed_dir"/*.puml)
if ((${#sources[@]} == 0)); then
    printf '%s\n' 'No PlantUML sources were found; refusing to pass.' >&2
    exit 1
fi

stale=0
for source in "${sources[@]}"; do
    name=$(basename "$source" .puml)
    for format in svg png; do
        if ! cmp -s "$committed_dir/$name.$format" "$temporary_dir/$name.$format"; then
            printf 'Diagram is missing or stale: %s/%s.%s\n' "$committed_dir" "$name" "$format" >&2
            stale=1
        fi
    done
done

rendered=("$temporary_dir"/*.svg)
if ((${#rendered[@]} != ${#sources[@]})); then
    printf 'Expected %d rendered diagrams, found %d\n' "${#sources[@]}" "${#rendered[@]}" >&2
    stale=1
fi

if ((stale != 0)); then
    printf 'Run `make diagrams` to refresh the committed assets.\n' >&2
    exit 1
fi
printf 'PlantUML assets are reproducible (%d diagrams).\n' "${#sources[@]}"
