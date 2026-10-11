#!/usr/bin/env bash
# Fails when a binary needs a newer glibc / libstdc++ than the oldest
# distributions we support (Ubuntu 22.04, Debian 12, Mint 21, Zorin 17).
#
#   scripts/check-portable.sh target/release/voice-changer [more binaries...]
#
# Limits: GLIBC <= 2.35, GLIBCXX <= 3.4.30, CXXABI <= 1.3.13.
set -euo pipefail
MAX_GLIBC="${MAX_GLIBC:-2.35}"
MAX_GLIBCXX="${MAX_GLIBCXX:-3.4.30}"
MAX_CXXABI="${MAX_CXXABI:-1.3.13}"

ver_gt() { [ "$(printf '%s\n%s\n' "$1" "$2" | sort -V | tail -n1)" != "$2" ]; }

status=0
for bin in "$@"; do
    echo "== $bin"
    syms=$(objdump -T "$bin" 2>/dev/null | grep -oE '(GLIBC|GLIBCXX|CXXABI)_[0-9.]+' | sort -u)
    glibc=$(echo "$syms" | grep -oE 'GLIBC_[0-9.]+' | sed 's/GLIBC_//' | sort -V | tail -n1 || true)
    glibcxx=$(echo "$syms" | grep -oE 'GLIBCXX_[0-9.]+' | sed 's/GLIBCXX_//' | sort -V | tail -n1 || true)
    cxxabi=$(echo "$syms" | grep -oE 'CXXABI_[0-9.]+' | sed 's/CXXABI_//' | sort -V | tail -n1 || true)
    echo "   needs glibc ${glibc:-none}, libstdc++ GLIBCXX ${glibcxx:-none}, CXXABI ${cxxabi:-none}"
    echo "   shared libraries: $(objdump -p "$bin" | awk '/NEEDED/ {printf "%s ", $2}')"
    if [ -n "$glibc" ] && ver_gt "$glibc" "$MAX_GLIBC"; then
        echo "   FAIL: glibc $glibc > $MAX_GLIBC (Ubuntu 22.04 / Debian 12 could not start it)"; status=1
    fi
    if [ -n "$glibcxx" ] && ver_gt "$glibcxx" "$MAX_GLIBCXX"; then
        echo "   FAIL: GLIBCXX $glibcxx > $MAX_GLIBCXX"; status=1
    fi
    if [ -n "$cxxabi" ] && ver_gt "$cxxabi" "$MAX_CXXABI"; then
        echo "   FAIL: CXXABI $cxxabi > $MAX_CXXABI"; status=1
    fi
done
[ $status -eq 0 ] && echo "portable: OK (runs on glibc >= $MAX_GLIBC)"
exit $status
