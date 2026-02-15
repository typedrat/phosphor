# GPU Rendering Pipeline

## Accumulation Buffer

The phosphor screen state is represented entirely on the GPU as a set of floating-point textures storing spectral energy per pixel.

### Spectral Band Layout

`SPECTRAL_BANDS` (compile-time constant, initially 16) bands spanning 380–780nm at ~25nm intervals. The band edges are computed from:

```
band_min[i] = 380.0 + i * (400.0 / SPECTRAL_BANDS)
band_max[i] = band_min[i] + (400.0 / SPECTRAL_BANDS)
```

Changing `SPECTRAL_BANDS` requires updating one Rust constant and one WGSL define — all buffer layouts, shader loops, and CIE integration weights derive from it automatically.

### Texture Layout

Each group of 4 spectral bands is packed into one RGBA32Float texture. For `SPECTRAL_BANDS = 16`, that's 4 textures per decay component per layer.

Full layout for a dual-layer phosphor:

- Fluorescence layer, fast decay: 4 textures (16 bands)
- Fluorescence layer, slow decay: 4 textures (16 bands)
- Phosphorescence layer, fast decay: 4 textures (16 bands)
- Phosphorescence layer, slow decay: 4 textures (16 bands)

**Total: 16 RGBA32Float textures.**

Memory at various resolutions:
| Resolution | Memory |
|------------|--------|
| 1920×1080 | ~528MB |
| 2560×1440 | ~943MB |
| 3840×2160 | ~2.1GB |

All within budget for a 32GB 5090. Single-layer phosphors use half this.

### Resolution

The accumulation buffer resolution represents the physical phosphor screen, independent of display resolution. Default: match the viewport's pixel resolution. Configurable via engineer mode (0.5x for performance, 2x for quality).

## Pass 1: Beam Write (Compute Shader)

**Input:** Array of `BeamSample` structs uploaded to a GPU storage buffer each frame.

**Operation:** For each sample, compute the Gaussian spot footprint (clamped to a tile of ~4σ radius). For each pixel in the tile:

1. Compute radial distance `r` from beam center
2. Evaluate spot profile: `I(r) = (1-h)·exp(-r²/2σ²) + h·exp(-r²/2σ_halo²)`
3. For each spectral band, deposit:
   ```
   energy = sample.intensity * I(r) * sample.dt * emission_weight[band]
   ```
4. Split energy between fast and slow decay textures according to the phosphor's `a_fast`/`a_slow` ratio
5. Write to accumulation textures using atomic add (or use a separate accumulation pass with blending)

**Dispatch:** One workgroup per beam sample, threads within the workgroup cooperatively cover the spot footprint tile.

### Line Segments

For vector/line input, the CPU pre-subdivides each segment into a series of BeamSamples along the path before uploading. The GPU sees only point samples — line math stays on the CPU to keep the compute shader simple.

## Pass 2: Decay (Compute Shader)

**Operation:** For every texel in every accumulation texture:

```
value *= exp(-dt / tau)
```

Where `dt` is the elapsed time since the last frame (from a uniform buffer), and `tau` is the decay time constant for that texture's component (fast or slow, fluorescence or phosphorescence layer).

**Dispatch:** One thread per texel. Trivially parallel, memory-bandwidth bound. Each texture is a separate dispatch (or a single dispatch iterating over bind groups).

**Precision:** Over many frames, repeated multiplication by `exp(-dt/tau)` accumulates floating-point error. For very long decay phosphors (P7 at 1500ms to 0.1%), values persist for thousands of frames. Periodic renormalization or using f32 with careful thresholding (zero out values below a perceptual threshold) prevents drift.

## Pass 3: Tonemap / Display (Fragment Shader)

A full-screen quad fragment shader that reads all accumulation textures and produces the final display output. Runs as a render pass to the swapchain (or an intermediate texture if multi-pass post-processing is needed).

### Stage 1: Spectral → CIE XYZ

For each pixel, sum all spectral bands (across both layers and both decay components) and integrate against the CIE 1931 color matching functions:

```
X = Σ_i energy[i] * x_bar[i] * band_width
Y = Σ_i energy[i] * y_bar[i] * band_width
Z = Σ_i energy[i] * z_bar[i] * band_width
```

The `x_bar`, `y_bar`, `z_bar` values are pre-integrated per band and stored in a uniform buffer (a `SPECTRAL_BANDS × 3` array).

### Stage 2: XYZ → Linear sRGB

Standard 3×3 matrix:

```
| R |   |  3.2406 -1.5372 -0.4986 |   | X |
| G | = | -0.9689  1.8758  0.0415 | × | Y |
| B |   |  0.0557 -0.2040  1.0570 |   | Z |
```

Values can exceed [0,1] for phosphor colors outside the sRGB gamut. Apply gamut mapping (luminance-preserving desaturation). In engineer mode, out-of-gamut pixels are flagged visually.

### Stage 3: Bloom

Simulates halation — light scattering through the glass faceplate.

1. Downsample the HDR linear RGB to 1/4 resolution
2. Apply a threshold (only bloom bright pixels)
3. Separable Gaussian blur (large radius, configurable)
4. Add the blurred result back to the full-resolution image at a configurable intensity

This is a multi-pass operation: downsample → blur-H → blur-V → composite. May use a separate render target.

### Stage 4: Glass & Curvature

- **Faceplate tint:** Multiply by an RGB transmission curve. Real CRT glass is slightly gray-green to improve contrast ratio. Configurable.
- **Screen curvature:** UV coordinate remap. Parameterized by radius of curvature (flat = infinite, classic CRT = ~1.5× screen diagonal). Applied as a distortion in the fragment shader.
- **Edge darkening:** Brightness falloff at screen edges from oblique viewing angle. `falloff = cos(θ)` where θ is the angle from screen normal to the pixel direction.

### Stage 5: Tonemapping

Compress HDR values to displayable [0, 1] range, then gamma-encode to sRGB.

Tonemap options:

- Reinhard (`L / (1 + L)`)
- Filmic (ACES-like S-curve)
- Exposure-based (simple multiply + clamp)

Parameters exposed in engineer mode: exposure, white point, contrast.

## Pipeline Orchestration

Each frame:

```
1. CPU: Drain ring buffer → upload BeamSamples to GPU staging buffer
2. GPU: Beam Write compute pass
3. GPU: Decay compute pass
4. GPU: Tonemap render pass → swapchain image
5. GPU: egui overlay pass → same swapchain image (side panel, controls drawn on top)
6. Present
```

Steps 2–5 are a single command buffer submission. The beam write and decay passes use storage textures with appropriate barriers between them. The tonemap pass reads the (now-decayed) accumulation textures as sampled textures.
