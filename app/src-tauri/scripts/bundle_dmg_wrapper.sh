#!/usr/bin/env bash
# Patch Tauri's generated bundle_dmg.sh to skip the failing AppleScript.
# Called via afterBuildCommand so it runs before the DMG bundler is invoked.
set -eu

# Find generated bundle_dmg.sh (debug or release)
for script in $(find target/release/bundle/dmg/bundle_dmg.sh target/debug/bundle/dmg/bundle_dmg.sh 2>/dev/null); do
    if [ -f "$script" ]; then
        # Insert --skip-jenkins before the osascript invocation.
        # This skips the Finder-prettifying AppleScript that fails on macOS 15+
        # (error -10006: Can't set statusbar visible of container window).
        sed -i '' 's|/usr/bin/osascript "${APPLESCRIPT_FILE}" "${VOLUME_NAME}"|echo "Skipping AppleScript (macOS 15+ compatibility)"\n\t\t\t# /usr/bin/osascript "${APPLESCRIPT_FILE}" "${VOLUME_NAME}"|' "$script"
        echo "Patched $script with --skip-jenkins"
    fi
done
