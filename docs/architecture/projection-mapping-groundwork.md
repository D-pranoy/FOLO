# Projection-mapping groundwork

Status: Accepted · Implementation: Not started

Projection mapping (projectors and displays as targets, warping, GPU projection) is **out of scope for version 1**. The items below are built now so it can be added later without reworking the core. Decided in [ADR-0025](../decisions/ADR-0025-projection-mapping-groundwork.md).

| # | Groundwork | Where it lives |
|---|---|---|
| G1 | Metric zone coordinates, room bounds, and the reference object's position and orientation | [Spatial model](spatial-model.md#zones) |
| G2 | A `display` target kind in the schema (resolution, `warp` as a 3×3 homography or mesh grid, `mask`), validated but never executed | [Configuration model](../specs/config-model.md) |
| G3 | Typed frames and pixel formats shared by sources, surfaces and outputs (`rgb8`, `rgb16`, `rgbf32`, a video format reserved) | [Pipeline](pipeline.md#frames-and-signals) |
| G4 | An abstract `Projector` interface, so a GPU implementation (wgpu) can sit next to the CPU gather projector | [Spatial model](spatial-model.md#mappings-and-projection) |
| G5 | Two provider output interfaces: `ElementOutput` (LED elements) and `RasterOutput` (frames; reserved for displays, NDI, Spout) | [Configuration model](../specs/config-model.md) |
| G6 | Monotonic microsecond timestamps end to end, with per-output latency offsets | [Pipeline](pipeline.md#frames-and-signals) |
| G7 | Per-target calibration records that can grow, for example for camera-based calibration | [Spatial model](spatial-model.md#targets) |
| G8 | Surface resolution independent of targets, plus area-average sampling on canvases (also used by screen sync) | [Spatial model](spatial-model.md#mappings-and-projection) |
