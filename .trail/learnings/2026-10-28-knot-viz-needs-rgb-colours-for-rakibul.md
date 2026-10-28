---
title: knot viz needs RGB colours for sigma and a folder-anchored ForceAtlas2 layout to show clusters
author: Rakibul
date: 2026-10-28
branch: audit-fixes
cost: { minutes: 19, tokens: 389208 }
id: l-8cae7d36
scope: repo
status: active
src: agent
touches:
  - src/ui/viz.rs
---

## Decided
The redesign of `knot viz` (src/ui/viz.rs) uses sigma.js on graphology. Its layout is ForceAtlas2 with a hidden anchor node per folder, tied to every symbol in that folder at weight 1.5, with linLogMode and outboundAttractionDistribution. The anchors are removed after the layout runs. That puts each folder in its own region, and calls between folders show as bridges. Folder colours step through hues by the golden angle (137.5°) and alternate lightness, so neighbouring folders never get near-identical colours. A prototype on this repo's 947 symbols checked all of this.

## Tried and ruled out
- Plain ForceAtlas2 with linLogMode and no anchors: one undifferentiated blob, because folders don't separate.
- Anchors at weight 3 without linLog: every folder curls into an artificial ring.
- Hues spread evenly around the wheel: src/cli, src/check and src/engine all came out pink.

## Watch out
Sigma's WebGL renderer can't parse `oklch()` colours and draws those nodes black. Canvas `fillStyle` passes oklch through unchanged, so reading it back doesn't help either. Convert each colour by painting one pixel and reading it back with getImageData.
