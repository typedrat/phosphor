# GPU Rendering Pipeline

## Accumulation Buffer

The phosphor screen state is represented entirely on the GPU as a flat storage buffer of scalar energy values. Each pixel has a variable number of layers depending on the active phosphor's decay term classification.

### Buffer Layout

A single `wgpu::Buffer` of `u32` values (bitcast to/from `f32` in shaders) indexed as:

```
index = layer * (width * height) + y * width + x
```

Layer assignment per phosphor (in order):

| Layer range                        | Tier | Content                                            |
| ---------------------------------- | ---- | -------------------------------------------------- |
| `0..slow_exp_count`                | 2    | One scalar energy per slow exponential term        |
| `slow_exp_count..slow_exp_count+2` | 3    | Peak energy + elapsed time (if power-law)          |
| Last layer                         | 1    | Instantaneous scalar energy (if any instant terms) |

### Memory

The scalar-layer approach dramatically reduces VRAM compared to spectral-per-layer textures. For example, P1 (2 slow exponentials) uses 2 layers instead of 32 (2 terms × 16 bands):

| Phosphor | Layers | Memory @ 1920×1080 |
| -------- | ------ | ------------------ |
| P1       | 2      | ~16 MB             |
| P31      | 3      | ~24 MB             |
| P15      | 1      | ~8 MB              |

### Resolution

The accumulation buffer resolution represents the physical phosphor screen, independent of display resolution. Default: match the viewport's pixel resolution. Configurable via engineer mode (0.25x–2x).

### Spectral Weighting

Emission weights are stored in `SpectralResolveParams` as `EmissionGroupGpu` structs (one per phosphor layer), not in the accumulation buffer. Each group contains 16 spectral weights packed as 4 × vec4, plus layer indices for that group's decay tiers. The spectral resolve pass multiplies scalar energy × emission weight per band during CIE integration.

## Pass 1: Beam Write (Compute Shader)

**Input:** Array of `BeamSample { x, y, intensity, dt }` structs uploaded to a GPU storage buffer.

**Operation:** For each sample, one workgroup computes the Gaussian spot footprint (tile of ~4σ radius). For each pixel in the tile:

1. Compute radial distance `r` from beam center
2. Evaluate spot profile: `I(r) = (1-h)·exp(-r²/2σ²) + h·exp(-r²/2σ_halo²)`
3. Compute base energy: `energy = intensity * I(r) * dt`
4. Deposit into tier-2 layers via `atomicAdd` (one scalar per slow exponential term)
5. Deposit into tier-3 power-law peak layer, reset elapsed time to 0
6. Deposit into tier-1 instant layer (includes analytical integration of fast exponentials: `Σ A·τ` energy boost)

**Dispatch:** One workgroup per beam sample, threads cover the spot footprint tile cooperatively.

### Line Segments

The CPU pre-subdivides each segment into a series of `BeamSample`s along the path before uploading. Arc-length resampling merges close samples to ~0.5× beam sigma spacing for uniform brightness. The GPU sees only point samples.

## Pass 2: Spectral Resolve (Fragment Shader)

**Runs before decay** so that newly deposited energy (including tier-1 instant emission) is displayed at full brightness for one frame.

**Operation:** Full-screen triangle fragment shader. For each pixel:

1. For each emission group (1 for single-layer, 2 for dual-layer phosphors):
   - Sum tier-2 slow exponential layers (scalar energy × emission weight per band)
   - Evaluate tier-3 power-law contribution: `peak * (α/(elapsed+α))^β` × emission weight
   - Add tier-1 instantaneous contribution × emission weight
2. Integrate spectral energy against pre-computed CIE 1931 weights: `X = Σ energy_band * x_bar_band` (and Y, Z)
3. XYZ → linear sRGB via standard 3×3 matrix (IEC 61966-2-1)
4. Luminance-preserving gamut mapping for out-of-gamut colors
5. Output linear sRGB + luminance (Y) as RGBA to the HDR buffer (Rgba32Float texture)

## Pass 3: Decay (Compute Shader)

**Runs after spectral resolve** so tier-1 instant layers are read before being cleared.

**Operation:** For every texel:

- **Tier 2 (slow exponentials):** `value *= exp(-dt/τ)` per term. Values below threshold are zeroed.
- **Tier 3 (power-law):** Increment elapsed time by `dt`. Zero out peak energy when `peak * (α/(elapsed+α))^β < threshold`.
- **Tier 1 (instantaneous):** Zero the instant energy layer (it was a one-frame contribution).

**Dispatch:** 16×16 workgroups covering the accumulation buffer dimensions.

## Pass 4: Faceplate Scatter (Compute Shaders)

Simulates halation — light scattering through the glass faceplate. Three sub-passes:

1. **Downsample:** HDR buffer → half-resolution texture A, applying a brightness threshold (only bloom bright pixels).
2. **Blur Horizontal:** Texture A → Texture B, separable Gaussian blur along X axis.
3. **Blur Vertical:** Texture B → Texture A, separable Gaussian blur along Y axis.

Configurable parameters: threshold, sigma (blur radius), intensity.

## Pass 5: Composite (Fragment Shader)

Full-screen triangle fragment shader combining all effects:

1. Sample HDR buffer (full-res sharp image)
2. Sample faceplate scatter texture (half-res bloom)
3. Blend: `color = hdr + scatter * scatter_intensity`
4. Apply glass faceplate tint (RGB multiply)
5. Apply screen curvature distortion (UV remap)
6. Apply edge brightness falloff (Lambertian cosine)
7. Tonemap to display range:
   - Reinhard: `L / (1 + L)`
   - ACES: filmic S-curve
   - Clamp: exposure multiply + clamp
   - None: HDR passthrough (when surface is Rgba16Float)

## Pass 6: egui Overlay (Render Pass)

The side panel / controls are rendered on top of the composite output. In detached mode, the viewport window skips egui entirely — pure GPU output only.

## Pipeline Orchestration

Each frame (single command buffer submission):

```
1. CPU: Generate BeamSamples from active input source
2. GPU: Beam Write compute pass → accumulation buffer
3. GPU: Spectral Resolve render pass → HDR buffer
4. GPU: Decay compute pass → accumulation buffer
5. GPU: Faceplate Scatter compute passes → scatter textures
6. GPU: Composite render pass → swapchain
7. GPU: egui overlay render pass → swapchain
8. GPU: Timestamp query resolve
9. Present
```

GPU profiling via timestamp queries measures the duration of each pass, displayed as a stacked timing plot in engineer mode.
