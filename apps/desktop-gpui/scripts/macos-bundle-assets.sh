#!/usr/bin/env bash

CAPWISE_ASSET_TREES=(backgrounds music)

copy_capwise_assets() {
  local source_assets="$1" bundle_assets="$2" tree
  mkdir -p "$bundle_assets"
  for tree in "${CAPWISE_ASSET_TREES[@]}"; do
    ditto "$source_assets/$tree" "$bundle_assets/$tree"
  done
}
