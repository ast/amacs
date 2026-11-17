#!/usr/bin/env bash
set -euo pipefail

name="${1:-world}"

greet() {
    local who="$1"
    echo "Hello, $who"
}

if [[ -n "$name" ]]; then
    greet "$name"
elif [[ -z "$name" ]]; then
    echo "empty"
else
    exit 1
fi

for file in *.txt; do
    while read -r line; do
        echo "$line"
    done < "$file"
done

case "$name" in
    a)
        echo "a"
        ;;
    *)
        echo "other"
        ;;
esac
