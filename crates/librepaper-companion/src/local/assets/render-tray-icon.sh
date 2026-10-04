#!/bin/sh
set -eu

asset_dir=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
magick -background none -density 192 "$asset_dir/tray-outline.svg" \
    -resize '64x64!' -depth 8 "PNG32:$asset_dir/tray-outline-64.png"
magick "$asset_dir/tray-outline-64.png" -depth 8 \
    "RGBA:$asset_dir/tray-outline-64.rgba"
