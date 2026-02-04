# Night Sky Screensaver Design

## Overview

A zen screensaver view for Yeehaw CLI featuring a starfield where stars gradually fade in and out at random positions. Accessible via global hotkey `v` from anywhere in the application.

## Core Concept

Stars appear at random positions in the terminal, transition through brightness levels (dim → bright → dim), then disappear. New stars spawn to maintain a sparse, serene density. The effect mimics a calm desert night sky.

## Data Model

```typescript
// Star brightness phases (indexes into character set)
type StarPhase = 0 | 1 | 2 | 3 | 4 | 5 | 6 | 7 | 8; // 0 = invisible, 4 = brightest

// Character sets (toggleable with 'c' key)
const CHAR_SETS = {
  classic: [' ', '.', '·', '+', '*', '+', '·', '.', ' '],
  unicode: [' ', '⋅', '∘', '∙', '●', '∙', '∘', '⋅', ' '],
  subtle:  [' ', '`', '.', "'", '*', "'", '.', '`', ' '],
};

interface Star {
  x: number;           // Column position
  y: number;           // Row position
  phase: StarPhase;    // Current brightness phase
  direction: 1 | -1;   // Fading in (+1) or out (-1)
  speed: number;       // Frames per phase transition (randomized)
  frameCount: number;  // Frames since last phase change
}
```

## Animation Parameters

| Parameter | Value | Notes |
|-----------|-------|-------|
| Frame rate | 10 FPS (100ms) | Smooth but low CPU |
| Target star count | 15-25 | Sparse, serene density |
| Frames per phase | 3-8 | 300-800ms per brightness step |
| Peak hold frames | 10-30 | 1-3 sec at maximum brightness |
| Spawn chance | 0.15/frame | ~1.5 new stars/sec when below target |

## Animation Loop

Each frame (100ms):
1. **Update existing stars** - Increment `frameCount`, advance phase when threshold reached
2. **Remove dead stars** - Stars that completed fade-out (phase 0, direction -1)
3. **Spawn new stars** - If below target density, spawn at random empty positions

Stars have randomized `speed` values for organic, non-synchronized pulsing.

## View Layout

```
┌─────────────────────────────────────────┐
│                                         │
│    .        *           ·               │
│         ·        +                      │
│                            .    *       │
│   *              ·                      │
│        +                    ·           │
│                                         │  ← Sky area (height - 4)
│             .         *                 │
│                  ·                      │
│                                         │
├─────────────────────────────────────────┤
│                                         │  ← Reserved for future
│                                         │    desert landscape (4 rows)
└─────────────────────────────────────────┘
```

## Hotkeys

### Global (added)
| Key | Action |
|-----|--------|
| `v` | Enter Night Sky visualizer |

### Night Sky View
| Key | Action |
|-----|--------|
| `q` / `Esc` | Exit to previous view |
| `c` | Cycle character set |
| `r` | Randomize star positions |

## File Changes

### New Files
- `src/views/NightSkyView.tsx` - The screensaver view component

### Modified Files
- `src/types.ts` - Add `{ type: 'night-sky' }` to `AppView` union
- `src/app.tsx` - Add view routing and global `v` hotkey
- `src/lib/hotkeys.ts` - Add `'night-sky'` scope and hotkey definitions

## Implementation Order

1. Add `'night-sky'` to `AppView` type union
2. Add hotkey scope and definitions to `hotkeys.ts`
3. Create `NightSkyView.tsx` with static star rendering
4. Add animation loop for star lifecycle
5. Wire up navigation in `app.tsx`
6. Test and tune timing values

## Future Enhancements

- Desert landscape at bottom (cacti, bumpy ground)
- Tumbleweed character that rolls across the ground
- Event banners triggered by webhooks/sessions
- Shooting stars for special events
- Configurable density/speed settings
