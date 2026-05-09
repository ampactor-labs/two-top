# Phase 15 Image Prompts

Use these when the actual image generation tool is available. Treat the
outputs as concept source, then manually clean frames into the 16-color
palette in `assets/palettes/two_top_16.gpl`.

## Character Pair Concept

Use case: stylized-concept
Asset type: pixel-art character concept sheet for a 1v1 portrait mobile
rollback brawler.
Primary request: two small demonic duelists for 2-Top, top-down / slight
three-quarter pixel art, readable at 24 by 24 px source size, one red
player and one cyan player, visibly different silhouettes, occult but
clean competitive-game readability.
Style: disciplined limited-palette pixel art, Hyper Light Drifter
readability, demonic-Duke tone, chunky silhouettes, no painterly blur.
Constraints: 16-color palette feel, transparent or flat background, no
text, no gore detail, no busy internal decoration, no color-only identity.
Must show: idle pose, throw windup pose, dash smear pose, hit pose, death
dissolve pose for both players.

## Boomerang and VFX Concept

Use case: stylized-concept
Asset type: pixel-art projectile and combat VFX concept sheet.
Primary request: bone-fang boomerang projectile, 12 by 12 px source
silhouette, plus flying trail, returning trail, hit burst, death burst,
and ambient ember particles for a competitive mobile pixel brawler.
Style: hard-edged chunky pixels, short geometric VFX, high gameplay
clarity, warm bone projectile brighter than the environment.
Constraints: no smoke clouds, no translucent haze, no blur, no large
fullscreen effects, no detailed background, no text.
Must show: projectile alone, 3 trail frames, 4 hit-burst frames, 6
death-burst frames, 4 ambient ember frames.

## Training Arena Concept

Use case: stylized-concept
Asset type: pixel-art arena tile and floor concept.
Primary request: portrait 1v1 duel arena floor for 2-Top, occult training
ring, open box arena, quiet dark stone floor, visible center lane, two
spawn marks, walls readable but lower priority than players and
projectiles.
Style: restrained demonic pixel art, low-contrast floor motifs, clean
competitive readability, top-down mobile game composition.
Constraints: do not use bright player/projectile colors in floor detail,
avoid dense texture in the center play lane, no large decorative card
frames, no text.
Must show: floor tile motif, wall edge/corner treatment, spawn marks,
subtle center duel ring.

## HUD and Touch Concept

Use case: stylized-concept
Asset type: pixel-art HUD/touch control concept sheet.
Primary request: compact HUD for a portrait 1v1 mobile brawler: score
pips, 30-second timer, countdown digits, match-over badge, and subtle
virtual stick / throw touch affordance.
Style: sharp minimal pixel UI, high contrast, readable over dark arena,
no ornate frame clutter.
Constraints: touch targets must respect 44 pt iOS / 48 dp Android
minimums in implementation; visible glyphs can be smaller with larger
hit regions. Do not rely on color alone. No explanatory text in the art.
Must show: red/cyan score pips, 3-2-1-GO countdown glyphs, match-over
badge, pressed/unpressed touch ring states.
