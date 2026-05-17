#!/bin/sh
# Remove the hifz launchd LaunchAgent. Leaves ~/.hifz/data and logs intact.
# Deterministic, single path, modern launchctl only.
set -eu

LABEL="com.hifz.server"
PLIST_DST="$HOME/Library/LaunchAgents/$LABEL.plist"
DOMAIN="gui/$(id -u)"

# bootout may legitimately find nothing loaded — that is success for an
# uninstall (idempotent). Any other failure is surfaced by set -e on the rm.
launchctl bootout "$DOMAIN/$LABEL" 2>/dev/null || true
rm -f "$PLIST_DST"

echo "hifz service uninstalled ($LABEL) — ~/.hifz/data and logs left intact"
