// ---------------------------------------------------------------------------
// Native SurfaceRules engine — exact replica of vanilla SurfaceSystem +
// SurfaceRules (Paper 1.21.8, mojmap), driven by worldpack surface_rule JSON.
//
// Parity contract (decompiled SurfaceRules.java / SurfaceSystem.java):
//   * column walk (buildSurface): topY = WORLD_SURFACE_WG heightmap + 1;
//     per y from topY down: air resets depth=0 & waterHeight=MIN;
//     fluid: waterHeight = y+1 (once); solid: stone-run bottom scan
//     (cache per contiguous run, "i8 >= i9" triggers rescan from y-1 down
//     to minY-1 until first non-stone, bottom = y2+1);
//     stoneDepthAbove = ++depth counter (reset on air),
//     stoneDepthBelow = y - stoneBottom + 1;
//     rule applies ONLY where block == defaultBlock (stone).
//   * isStone = !air && fluidState empty.
//   * getSurfaceDepth(x,z) = (int)(surface_noise.value(x,0,z) * 2.75 + 3.0
//     + surfaceFactory.at(x,0,z).nextDouble() * 0.25)   [double math]
//   * getSurfaceSecondary(x,z) = surface_secondary noise at (x,0,z)
//   * getMinSurfaceLevel per column: 4 preliminarySurfaceLevel samples on the
//     16-block cell corners (x&~15, z&~15 grid), Mth.lerp2 over
//     ((x&15)/16f, (z&15)/16f) as f32, floor(), + surfaceDepth - 8.
//   * Lazy conditions cache per (XZ) or per Y within the walk (lastUpdate).
//   * bandlands: clayBands[192] built once from
//     surfaceFactory.fromHashOf("minecraft:clay_bands") — generateBands:
//     fill TERRACOTTA; walk i with nextInt(5)+1 gaps -> ORANGE;
//     makeBands(x1: YELLOW count 6..15, len 1+rnd(3)); BROWN size 2; RED size 1;
//     white head band: n = nextIntBetweenInclusive(9,15) entries spaced
//     nextInt(16)+4, WHITE_TERRACOTTA with light_gray neighbours on nextBoolean.
//     getBand(x,y,z) = bands[(y + round(clay_bands_offset.value(x,0,z)*4) + 192) % 192]
//   * temperature: biome.coldEnoughToSnow(pos, seaLevel) =
//     getHeightAdjustedTemperature < 0.15f:
//       f = modifier.modify(base) [NONE: f; FROZEN: clamp]
//       i = seaLevel + 17; if y > i:
//         f - (temperature_noise.value(x/8, z/8)*8 + y - i) * 0.05 / 40
//       (noise arg as f32 casts: (float)x / 8.0f)
//   * vertical_gradient: y <= trueBelow -> true; y >= falseAbove -> false;
//     else rnd = factory(random_name).at(x,y,z); map(y, trueB, falseA, 1,0)
//     as double; rnd.nextFloat() (f32) < d.
//   * paper:optionally_flat_bedrock_condition_source == vertical_gradient
//     when generate-flat-bedrock=false (bench worlds).
//   * noise_threshold: noise.value(x,0,z) in [min, max] (LazyXZ).
//   * stone_depth: above/below + offset + surfaceDepth(add) +
//     secondaryRange mapped from surfaceSecondary [-1,1] -> [0,range].
//   * y_above: y + (addSD ? stoneDepthAbove:0) >= anchorY + surfaceDepth*mult
//   * water: waterHeight==MIN -> true (no water above);
//     else y + (addSD?stoneDepthAbove:0) >= waterHeight + offset + sd*mult
//   * hole: stoneDepthAbove <= 0; steep: heightmap z-neigh >= cur+4 (z then x).
//   * biome: biome at (x, y>>2 quart grid) in set (vanilla biomeManager).
// ---------------------------------------------------------------------------
