#!/usr/bin/env bash
set -euo pipefail

root_dir=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)
output_dir=${DIAGRAM_OUTPUT_DIR:-"$root_dir/diagrams"}
mkdir -p "$output_dir"

if [[ -n "${PLANTUML_JAR:-}" ]]; then
    if [[ ! -f "$PLANTUML_JAR" ]]; then
        printf 'PlantUML jar does not exist: %s\n' "$PLANTUML_JAR" >&2
        exit 2
    fi
    renderer=("${PLANTUML_JAVA:-java}" -jar "$PLANTUML_JAR")
elif command -v plantuml >/dev/null 2>&1; then
    renderer=(plantuml)
else
    printf '%s\n' 'Set PLANTUML_JAR or install plantuml; no renderer was found.' >&2
    exit 127
fi

shopt -s nullglob
sources=("$root_dir"/diagrams/*.puml)
if ((${#sources[@]} == 0)); then
    printf '%s\n' 'No PlantUML sources were found in diagrams/.' >&2
    exit 1
fi

for source in "${sources[@]}"; do
    "${renderer[@]}" -tsvg -o "$output_dir" "$source"
    "${renderer[@]}" -tpng -o "$output_dir" "$source"
done
printf 'Rendered %d diagrams in %s\n' "${#sources[@]}" "$output_dir"
