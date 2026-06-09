#!/bin/sh
# kamaji uninstaller. Usage:
#   curl -fsSL https://raw.githubusercontent.com/alveflo/kamaji/main/uninstall.sh | sh
#
# Removes the kamaji + kamajid binaries, stops a running daemon, and removes the
# systemd user service if one was installed. User data (the board database,
# config, cache) is KEPT by default — pass --purge to delete it too:
#   curl -fsSL .../uninstall.sh | sh -s -- --purge
#
# Override the install directory with KAMAJI_INSTALL_DIR (default: ~/.local/bin),
# matching install.sh.
set -eu

PURGE=0
for arg in "$@"; do
  case "$arg" in
    --purge) PURGE=1 ;;
    -h|--help)
      printf 'usage: uninstall.sh [--purge]\n  --purge  also delete config/data/cache (the board database)\n'
      exit 0 ;;
    *) printf 'error: unknown option: %s\n' "$arg" >&2; exit 1 ;;
  esac
done

err() { printf 'error: %s\n' "$1" >&2; exit 1; }

# Resolve the install directory the same way install.sh does.
if [ -n "${KAMAJI_INSTALL_DIR:-}" ]; then
  INSTALL_DIR="$KAMAJI_INSTALL_DIR"
elif [ -n "${HOME:-}" ]; then
  INSTALL_DIR="$HOME/.local/bin"
else
  err "HOME is not set; set KAMAJI_INSTALL_DIR to the directory kamaji was installed to"
fi

# XDG base dirs (Linux and macOS both use these — see kamaji-core::paths).
CONFIG_DIR="${XDG_CONFIG_HOME:-${HOME:-}/.config}/kamaji"
DATA_DIR="${XDG_DATA_HOME:-${HOME:-}/.local/share}/kamaji"
CACHE_DIR="${XDG_CACHE_HOME:-${HOME:-}/.cache}/kamaji"
# Daemon pidfile: $XDG_RUNTIME_DIR, else $XDG_CACHE_HOME, else ~/.cache — + /kamaji.
RUNTIME_BASE="${XDG_RUNTIME_DIR:-${XDG_CACHE_HOME:-${HOME:-}/.cache}}"
PID_FILE="${RUNTIME_BASE}/kamaji/kamajid.pid"

# 1. Stop a running daemon (pidfile first, then by process name as a fallback).
if [ -f "$PID_FILE" ]; then
  pid="$(tr -dc '0-9' < "$PID_FILE" 2>/dev/null || true)"
  if [ -n "${pid:-}" ] && kill "$pid" 2>/dev/null; then
    printf 'Stopped running kamajid (pid %s)\n' "$pid"
  fi
elif command -v pkill >/dev/null 2>&1 && pkill -x kamajid 2>/dev/null; then
  printf 'Stopped running kamajid\n'
fi

# 2. Remove the systemd user service, if one was installed (Linux + systemd).
if command -v systemctl >/dev/null 2>&1; then
  UNIT_DIR="${XDG_CONFIG_HOME:-${HOME:-}/.config}/systemd/user"
  UNIT_DEST="${UNIT_DIR}/kamajid.service"
  if [ -f "$UNIT_DEST" ]; then
    systemctl --user disable --now kamajid.service 2>/dev/null || true
    rm -f "$UNIT_DEST"
    systemctl --user daemon-reload 2>/dev/null || true
    printf 'Removed systemd user service (kamajid.service)\n'
  fi
fi

# 3. Remove the installed binaries.
removed_any=0
for bin in kamaji kamajid; do
  path="${INSTALL_DIR}/${bin}"
  if [ -e "$path" ]; then
    rm -f "$path"
    printf 'Removed %s\n' "$path"
    removed_any=1
  fi
done
if [ "$removed_any" -eq 0 ]; then
  printf 'No kamaji binaries found in %s\n' "$INSTALL_DIR"
  # If kamaji is still on PATH it was installed somewhere else — point the user there.
  if command -v kamaji >/dev/null 2>&1; then
    printf 'Note: a "kamaji" is still on your PATH at %s — remove it manually or set KAMAJI_INSTALL_DIR.\n' \
      "$(command -v kamaji)"
  fi
fi

# 4. Data: kept by default, deleted with --purge.
if [ "$PURGE" -eq 1 ]; then
  for d in "$CONFIG_DIR" "$DATA_DIR" "$CACHE_DIR"; do
    if [ -d "$d" ]; then
      rm -rf "$d"
      printf 'Purged %s\n' "$d"
    fi
  done
  printf '\nkamaji fully uninstalled.\n'
else
  printf '\nkamaji uninstalled. Your data was kept:\n'
  [ -d "$CONFIG_DIR" ] && printf '  config: %s\n' "$CONFIG_DIR"
  [ -d "$DATA_DIR" ]   && printf '  data:   %s\n' "$DATA_DIR"
  [ -d "$CACHE_DIR" ]  && printf '  cache:  %s\n' "$CACHE_DIR"
  printf 'Re-run with --purge to delete it.\n'
fi
