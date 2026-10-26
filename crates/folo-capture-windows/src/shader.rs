//! The downscaling shader (ADR-0037 D18): HLSL embedded here and compiled at
//! run time by the in-box `d3dcompiler_47`, so nothing is needed at build time
//! and no library ships.
//!
//! One pass draws the whole frame: each pixel of the output averages the
//! source texels under it, each weighted by how much of it the output pixel
//! covers (the box filter of [`folo_capture_core::color::downscale`]), in
//! linear light. An 8-bit desktop is sRGB-decoded first. An HDR desktop is
//! scRGB (linear already): the average is tone-mapped against the SDR white
//! level ([`crate::tonemap`]). The result is sRGB-encoded.

use windows::Win32::Graphics::Direct3D::Fxc::{D3DCOMPILE_ENABLE_STRICTNESS, D3DCompile};
use windows::Win32::Graphics::Direct3D::ID3DBlob;
use windows::core::{PCSTR, s};

use crate::fault::Fault;

/// Entry point of the vertex shader.
pub const VERTEX_ENTRY: PCSTR = s!("vs_main");
/// Entry point of the pixel shader.
pub const PIXEL_ENTRY: PCSTR = s!("ps_main");
/// Shader model 4.0 runs on feature level 10.0 and up, which every GPU and WARP has.
pub const VERTEX_TARGET: PCSTR = s!("vs_4_0");
pub const PIXEL_TARGET: PCSTR = s!("ps_4_0");

/// The size of the constant buffer, in bytes; `Constants` in `gpu.rs` has this layout.
pub const CONSTANTS_BYTES: u32 = 32;

pub const HLSL: &str = r"
cbuffer Params : register(b0)
{
    uint2 src_size;
    uint2 dst_size;
    float hdr_scale; // 80 / the SDR white level in nits; read when hdr is not 0
    uint hdr;        // 0: 8-bit sRGB source. 1: scRGB (linear, 1.0 = 80 nits) source
    float2 padding;
};

Texture2D<float4> source : register(t0);

// One triangle that covers the whole target.
float4 vs_main(uint id : SV_VertexID) : SV_Position
{
    float2 corner = float2((id << 1) & 2, id & 2);
    return float4(corner * float2(2, -2) + float2(-1, 1), 0, 1);
}

float3 srgb_to_linear(float3 encoded)
{
    float3 low = encoded / 12.92;
    float3 high = pow((encoded + 0.055) / 1.055, 2.4);
    return lerp(high, low, step(encoded, 0.04045));
}

float3 linear_to_srgb(float3 light)
{
    float3 low = light * 12.92;
    float3 high = 1.055 * pow(max(light, 0.0031308), 1.0 / 2.4) - 0.055;
    return lerp(high, low, step(light, 0.0031308));
}

// Brighter than SDR white is divided by its brightest component, which keeps the hue.
float3 tone_map(float3 scrgb)
{
    float3 scaled = max(scrgb * hdr_scale, 0);
    float brightest = max(scaled.r, max(scaled.g, scaled.b));
    return brightest > 1 ? scaled / brightest : scaled;
}

float4 ps_main(float4 position : SV_Position) : SV_Target
{
    float2 scale = float2(src_size) / float2(dst_size);
    float2 from = floor(position.xy) * scale;
    float2 to = from + scale;
    uint2 first = uint2(floor(from));
    uint2 last = min(uint2(ceil(to)), src_size);
    float3 sum = 0;
    for (uint y = first.y; y < last.y; ++y)
    {
        float weight_y = max(min(to.y, y + 1.0) - max(from.y, (float)y), 0);
        for (uint x = first.x; x < last.x; ++x)
        {
            float weight_x = max(min(to.x, x + 1.0) - max(from.x, (float)x), 0);
            float3 texel = source.Load(int3(x, y, 0)).rgb;
            texel = hdr != 0 ? texel : srgb_to_linear(texel);
            sum += texel * (weight_x * weight_y);
        }
    }
    float3 mean = sum / (scale.x * scale.y);
    if (hdr != 0)
    {
        mean = tone_map(mean);
    }
    return float4(linear_to_srgb(saturate(mean)), 1);
}
";

/// Compiles `source` for `entry` and `target`.
///
/// # Errors
///
/// [`Fault::Other`] with the compiler's messages when the HLSL doesn't compile
/// or `d3dcompiler_47.dll` can't be loaded.
#[allow(unsafe_code)] // D3DCompile and reading the blobs it returns
pub fn compile(source: &str, entry: PCSTR, target: PCSTR) -> Result<Vec<u8>, Fault> {
    let mut code: Option<ID3DBlob> = None;
    let mut messages: Option<ID3DBlob> = None;
    // SAFETY: `source` is valid for `source.len()` bytes for the call; the name,
    // entry and target are NUL-terminated literals; the two out pointers are to
    // locals that live for the call; no defines or includes are passed.
    let compiled = unsafe {
        D3DCompile(
            source.as_ptr().cast(),
            source.len(),
            s!("folo-capture.hlsl"),
            None,
            None,
            entry,
            target,
            D3DCOMPILE_ENABLE_STRICTNESS,
            0,
            &raw mut code,
            Some(&raw mut messages),
        )
    };
    match (compiled, code) {
        (Ok(()), Some(blob)) => Ok(blob_bytes(&blob)),
        (Ok(()), None) => Err(Fault::Other(
            "the shader compiler returned no code".to_owned(),
        )),
        (Err(error), _) => {
            let detail = messages.map(|blob| blob_bytes(&blob)).unwrap_or_default();
            Err(Fault::Other(format!(
                "the shader didn't compile: {}: {}",
                error.message(),
                String::from_utf8_lossy(&detail)
                    .trim_end_matches('\0')
                    .trim()
            )))
        }
    }
}

#[allow(unsafe_code)] // reading a blob's buffer
fn blob_bytes(blob: &ID3DBlob) -> Vec<u8> {
    // SAFETY: a blob's pointer is valid for its size while the blob is alive,
    // and `blob` is borrowed for the whole copy.
    unsafe {
        std::slice::from_raw_parts(blob.GetBufferPointer().cast::<u8>(), blob.GetBufferSize())
            .to_vec()
    }
}
