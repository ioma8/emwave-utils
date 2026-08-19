#!/bin/bash
# Launch emWave Pro with the HID hook dylib injected.
# Log: /tmp/hid_hook.log (append mode)
cd "$(dirname "$0")"
rm -f /tmp/hid_hook.log
export DYLD_INSERT_LIBRARIES="$(pwd)/hook_hid.dylib"
echo "launching emWave Pro with hook; log -> /tmp/hid_hook.log"
"/Applications/emWave Pro.app/Contents/MacOS/emWaveMac"
