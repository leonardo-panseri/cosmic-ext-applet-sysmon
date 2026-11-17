#!/bin/sh
# Fails if BINARY needs a GLIBC_ symbol version newer than MAX (default 2.34): the probe runs
# with the host's glibc, and ADR 0002 sets 2.34 as the floor. Needs readelf (binutils) and sort -V.
# Usage: tools/check-glibc.sh BINARY [MAX]
set -eu
binary=$1
max=${2:-2.34}
versions=$(readelf --version-info --wide "$binary" | grep -o 'GLIBC_[0-9][0-9.]*' | sed 's/^GLIBC_//' | sort -uV)
newest=$(printf '%s\n' "$versions" | tail -n 1)
if [ -z "$newest" ]; then
    echo "check-glibc: no GLIBC_ symbol versions in $binary" >&2
    exit 1
fi
if [ "$(printf '%s\n%s\n' "$newest" "$max" | sort -V | tail -n 1)" != "$max" ]; then
    echo "check-glibc: $binary needs GLIBC_$newest, newer than the GLIBC_$max floor:" >&2
    readelf --version-info --wide "$binary" | grep -o "GLIBC_$newest" | sort -u >&2
    exit 1
fi
echo "check-glibc: $binary needs at most GLIBC_$newest (floor GLIBC_$max)"
