//! The GPU side of the backend (ADR-0015 D5, ADR-0037 D18): a Direct3D 11
//! device, and the [`Downscaler`] that turns a texture of the desktop into at
//! most 480×270 sRGB pixels before anything is read back to the CPU.
//!
//! The downscaler takes any 2D texture with a shader-resource binding, so
//! the grabbers use it on what they capture and the tests use it on the WARP
//! software device with synthetic textures.

use std::mem::size_of;

use windows::Win32::Foundation::HMODULE;
use windows::Win32::Graphics::Direct3D::{
    D3D_DRIVER_TYPE, D3D_DRIVER_TYPE_HARDWARE, D3D_DRIVER_TYPE_UNKNOWN, D3D_DRIVER_TYPE_WARP,
    D3D_FEATURE_LEVEL, D3D_FEATURE_LEVEL_10_0, D3D_FEATURE_LEVEL_10_1, D3D_FEATURE_LEVEL_11_0,
    D3D_FEATURE_LEVEL_11_1, D3D_PRIMITIVE_TOPOLOGY_TRIANGLELIST,
};
use windows::Win32::Graphics::Direct3D11::{
    D3D11_BIND_CONSTANT_BUFFER, D3D11_BIND_RENDER_TARGET, D3D11_BIND_SHADER_RESOURCE,
    D3D11_BUFFER_DESC, D3D11_CPU_ACCESS_READ, D3D11_CREATE_DEVICE_BGRA_SUPPORT, D3D11_MAP_READ,
    D3D11_MAPPED_SUBRESOURCE, D3D11_SDK_VERSION, D3D11_SUBRESOURCE_DATA, D3D11_TEXTURE2D_DESC,
    D3D11_USAGE_DEFAULT, D3D11_USAGE_STAGING, D3D11_VIEWPORT, D3D11CreateDevice, ID3D11Buffer,
    ID3D11Device, ID3D11DeviceContext, ID3D11PixelShader, ID3D11RenderTargetView,
    ID3D11ShaderResourceView, ID3D11Texture2D, ID3D11VertexShader,
};
use windows::Win32::Graphics::Dxgi::Common::{
    DXGI_FORMAT, DXGI_FORMAT_R8G8B8A8_UNORM, DXGI_SAMPLE_DESC,
};
use windows::Win32::Graphics::Dxgi::IDXGIAdapter;

use crate::fault::Fault;
use crate::shader;

/// What the shader's constant buffer holds (`Params` in the HLSL).
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq)]
struct Constants {
    source_size: [u32; 2],
    target_size: [u32; 2],
    /// [`crate::tonemap::scale_for`] of the SDR white level.
    hdr_scale: f32,
    /// 0 for an 8-bit sRGB source, 1 for an scRGB one.
    hdr: u32,
    padding: [f32; 2],
}

/// A device and its immediate context. The context must be used by one thread at a time.
#[derive(Clone)]
pub struct Gpu {
    pub device: ID3D11Device,
    pub context: ID3D11DeviceContext,
}

impl Gpu {
    /// A device on `adapter`: the one that drives the output being captured, since
    /// Desktop Duplication fails when the device is on another GPU.
    ///
    /// # Errors
    ///
    /// A [`Fault`] when the device can't be created.
    pub fn on_adapter(adapter: &IDXGIAdapter) -> Result<Self, Fault> {
        Self::create(Some(adapter), D3D_DRIVER_TYPE_UNKNOWN)
    }

    /// A device on the default hardware adapter, for outputs no adapter was found for.
    ///
    /// # Errors
    ///
    /// A [`Fault`] when the device can't be created.
    pub fn hardware() -> Result<Self, Fault> {
        Self::create(None, D3D_DRIVER_TYPE_HARDWARE)
    }

    /// The WARP software device: it needs no GPU, so shader tests run on any machine.
    ///
    /// # Errors
    ///
    /// A [`Fault`] when Windows has no WARP.
    pub fn warp() -> Result<Self, Fault> {
        Self::create(None, D3D_DRIVER_TYPE_WARP)
    }

    #[allow(unsafe_code)] // D3D11CreateDevice with out pointers
    fn create(adapter: Option<&IDXGIAdapter>, driver: D3D_DRIVER_TYPE) -> Result<Self, Fault> {
        const LEVELS: [D3D_FEATURE_LEVEL; 4] = [
            D3D_FEATURE_LEVEL_11_1,
            D3D_FEATURE_LEVEL_11_0,
            D3D_FEATURE_LEVEL_10_1,
            D3D_FEATURE_LEVEL_10_0,
        ];
        let mut device = None;
        let mut context = None;
        // SAFETY: the out pointers are to locals that live for the call; the adapter
        // is borrowed for the call, and no software module is passed.
        unsafe {
            D3D11CreateDevice(
                adapter,
                driver,
                HMODULE::default(),
                D3D11_CREATE_DEVICE_BGRA_SUPPORT,
                Some(&LEVELS),
                D3D11_SDK_VERSION,
                Some(&raw mut device),
                None,
                Some(&raw mut context),
            )?;
        }
        match (device, context) {
            (Some(device), Some(context)) => Ok(Self { device, context }),
            _ => Err(Fault::Other("Direct3D made no device".to_owned())),
        }
    }

    /// Makes an empty 2D texture.
    ///
    /// # Errors
    ///
    /// A [`Fault`] when the device refuses (a lost device, a size it can't hold).
    pub fn texture(&self, desc: &D3D11_TEXTURE2D_DESC) -> Result<ID3D11Texture2D, Fault> {
        self.create_texture(desc, None)
    }

    /// Makes a 2D texture of `desc.Height` rows of `pitch` bytes taken from `bytes`.
    ///
    /// # Errors
    ///
    /// [`Fault::Other`] when `bytes` is shorter than the rows, and a [`Fault`]
    /// when the device refuses.
    pub fn texture_from(
        &self,
        desc: &D3D11_TEXTURE2D_DESC,
        bytes: &[u8],
        pitch: u32,
    ) -> Result<ID3D11Texture2D, Fault> {
        let needed = u64::from(pitch) * u64::from(desc.Height);
        if (bytes.len() as u64) < needed {
            return Err(Fault::Other(format!(
                "a texture of {needed} bytes was given {}",
                bytes.len()
            )));
        }
        let data = D3D11_SUBRESOURCE_DATA {
            pSysMem: bytes.as_ptr().cast(),
            SysMemPitch: pitch,
            SysMemSlicePitch: 0,
        };
        self.create_texture(desc, Some(&data))
    }

    #[allow(unsafe_code)] // CreateTexture2D with pointers to locals
    fn create_texture(
        &self,
        desc: &D3D11_TEXTURE2D_DESC,
        data: Option<&D3D11_SUBRESOURCE_DATA>,
    ) -> Result<ID3D11Texture2D, Fault> {
        let mut texture = None;
        // SAFETY: `desc` and `data` are borrowed for the call and the out pointer is to a
        // local; `data` points into a slice that `texture_from` checked is long enough and
        // that its caller keeps alive for the call.
        unsafe {
            self.device.CreateTexture2D(
                desc,
                data.map(std::ptr::from_ref),
                Some(&raw mut texture),
            )?;
        }
        texture.ok_or_else(|| Fault::Other("Direct3D made no texture".to_owned()))
    }

    /// The description of a texture.
    #[must_use]
    #[allow(unsafe_code)] // GetDesc writes through a pointer
    pub fn describe(texture: &ID3D11Texture2D) -> D3D11_TEXTURE2D_DESC {
        let mut desc = D3D11_TEXTURE2D_DESC::default();
        // SAFETY: the pointer is to a local that lives for the call.
        unsafe { texture.GetDesc(&raw mut desc) };
        desc
    }
}

/// The render target a frame is drawn on, and the staging texture it is read back from.
struct Target {
    width: u16,
    height: u16,
    texture: ID3D11Texture2D,
    view: ID3D11RenderTargetView,
    staging: ID3D11Texture2D,
}

/// Downscales textures on the GPU and reads the small result back.
pub struct Downscaler {
    gpu: Gpu,
    vertex_shader: ID3D11VertexShader,
    pixel_shader: ID3D11PixelShader,
    constants: ID3D11Buffer,
    target: Option<Target>,
}

impl Downscaler {
    /// Compiles the shaders and makes the constant buffer on `gpu`.
    ///
    /// # Errors
    ///
    /// A [`Fault`] when the shaders don't compile or the device refuses them.
    pub fn new(gpu: Gpu) -> Result<Self, Fault> {
        let vertex_code =
            shader::compile(shader::HLSL, shader::VERTEX_ENTRY, shader::VERTEX_TARGET)?;
        let pixel_code = shader::compile(shader::HLSL, shader::PIXEL_ENTRY, shader::PIXEL_TARGET)?;
        let (vertex_shader, pixel_shader) = Self::create_shaders(&gpu, &vertex_code, &pixel_code)?;
        let constants = Self::create_constants(&gpu)?;
        Ok(Self {
            gpu,
            vertex_shader,
            pixel_shader,
            constants,
            target: None,
        })
    }

    #[allow(unsafe_code)] // CreateVertexShader and CreatePixelShader with out pointers
    fn create_shaders(
        gpu: &Gpu,
        vertex_code: &[u8],
        pixel_code: &[u8],
    ) -> Result<(ID3D11VertexShader, ID3D11PixelShader), Fault> {
        let (mut vertex, mut pixel) = (None, None);
        // SAFETY: the bytecode slices are borrowed for the calls and the out pointers
        // are to locals that live for them.
        unsafe {
            gpu.device
                .CreateVertexShader(vertex_code, None, Some(&raw mut vertex))?;
            gpu.device
                .CreatePixelShader(pixel_code, None, Some(&raw mut pixel))?;
        }
        match (vertex, pixel) {
            (Some(vertex), Some(pixel)) => Ok((vertex, pixel)),
            _ => Err(Fault::Other("Direct3D made no shader".to_owned())),
        }
    }

    #[allow(unsafe_code)] // CreateBuffer with pointers to locals
    fn create_constants(gpu: &Gpu) -> Result<ID3D11Buffer, Fault> {
        debug_assert_eq!(size_of::<Constants>(), shader::CONSTANTS_BYTES as usize);
        let desc = D3D11_BUFFER_DESC {
            ByteWidth: shader::CONSTANTS_BYTES,
            Usage: D3D11_USAGE_DEFAULT,
            BindFlags: D3D11_BIND_CONSTANT_BUFFER.0.cast_unsigned(),
            CPUAccessFlags: 0,
            MiscFlags: 0,
            StructureByteStride: 0,
        };
        let mut buffer = None;
        // SAFETY: `desc` is borrowed for the call and the out pointer is to a local.
        unsafe {
            gpu.device
                .CreateBuffer(&raw const desc, None, Some(&raw mut buffer))?;
        }
        buffer.ok_or_else(|| Fault::Other("Direct3D made no buffer".to_owned()))
    }

    /// Downscales `source` to `size` (width, height) and returns `width × height × 3`
    /// bytes of sRGB rgb8. `hdr_scale` is `Some` for an scRGB (FP16) source, as
    /// [`crate::tonemap::scale_for`] gives it, and `None` for an 8-bit sRGB one.
    ///
    /// `source` needs a shader-resource binding, and must be in the format of an 8-bit
    /// BGRA or RGBA desktop or of an FP16 one.
    ///
    /// # Errors
    ///
    /// A [`Fault`] when the device refuses, or has been lost.
    pub fn downscale(
        &mut self,
        source: &ID3D11Texture2D,
        size: (u16, u16),
        hdr_scale: Option<f32>,
    ) -> Result<Vec<u8>, Fault> {
        let source_desc = Gpu::describe(source);
        let view = self.shader_view(source)?;
        self.ensure_target(size)?;
        let constants = Constants {
            source_size: [source_desc.Width, source_desc.Height],
            target_size: [u32::from(size.0), u32::from(size.1)],
            hdr_scale: hdr_scale.unwrap_or(1.0),
            hdr: u32::from(hdr_scale.is_some()),
            padding: [0.0; 2],
        };
        self.draw(&view, &constants);
        self.read_back()
    }

    #[allow(unsafe_code)] // CreateShaderResourceView with an out pointer
    fn shader_view(&self, source: &ID3D11Texture2D) -> Result<ID3D11ShaderResourceView, Fault> {
        let mut view = None;
        // SAFETY: the texture is borrowed for the call, a null description gives the
        // texture's own format, and the out pointer is to a local.
        unsafe {
            self.gpu
                .device
                .CreateShaderResourceView(source, None, Some(&raw mut view))?;
        }
        view.ok_or_else(|| Fault::Other("Direct3D made no shader resource view".to_owned()))
    }

    fn ensure_target(&mut self, size: (u16, u16)) -> Result<(), Fault> {
        if self
            .target
            .as_ref()
            .is_some_and(|target| (target.width, target.height) == size)
        {
            return Ok(());
        }
        self.target = Some(self.create_target(size)?);
        Ok(())
    }

    #[allow(unsafe_code)] // CreateRenderTargetView with an out pointer
    fn create_target(&self, size: (u16, u16)) -> Result<Target, Fault> {
        let mut desc = D3D11_TEXTURE2D_DESC {
            Width: u32::from(size.0),
            Height: u32::from(size.1),
            MipLevels: 1,
            ArraySize: 1,
            Format: DXGI_FORMAT_R8G8B8A8_UNORM,
            SampleDesc: DXGI_SAMPLE_DESC {
                Count: 1,
                Quality: 0,
            },
            Usage: D3D11_USAGE_DEFAULT,
            BindFlags: D3D11_BIND_RENDER_TARGET.0.cast_unsigned(),
            CPUAccessFlags: 0,
            MiscFlags: 0,
        };
        let texture = self.gpu.texture(&desc)?;
        desc.Usage = D3D11_USAGE_STAGING;
        desc.BindFlags = 0;
        desc.CPUAccessFlags = D3D11_CPU_ACCESS_READ.0.cast_unsigned();
        let staging = self.gpu.texture(&desc)?;
        let mut view = None;
        // SAFETY: the texture is borrowed for the call and the out pointer is to a local.
        unsafe {
            self.gpu
                .device
                .CreateRenderTargetView(&texture, None, Some(&raw mut view))?;
        }
        let view =
            view.ok_or_else(|| Fault::Other("Direct3D made no render target view".to_owned()))?;
        Ok(Target {
            width: size.0,
            height: size.1,
            texture,
            view,
            staging,
        })
    }

    /// Draws the frame. The context's calls report no errors; a lost device shows at the read-back.
    #[allow(unsafe_code)] // the immediate context's state and draw calls
    fn draw(&self, view: &ID3D11ShaderResourceView, constants: &Constants) {
        let Some(target) = &self.target else { return };
        let context = &self.gpu.context;
        let viewport = D3D11_VIEWPORT {
            TopLeftX: 0.0,
            TopLeftY: 0.0,
            Width: f32::from(target.width),
            Height: f32::from(target.height),
            MinDepth: 0.0,
            MaxDepth: 1.0,
        };
        // SAFETY: every object is alive for the calls (owned by `self`, `target` or
        // borrowed), `constants` is `CONSTANTS_BYTES` of plain data that UpdateSubresource
        // copies before returning, and the context is used by this thread only.
        unsafe {
            context.UpdateSubresource(
                &self.constants,
                0,
                None,
                std::ptr::from_ref(constants).cast(),
                0,
                0,
            );
            context.IASetPrimitiveTopology(D3D_PRIMITIVE_TOPOLOGY_TRIANGLELIST);
            context.VSSetShader(&self.vertex_shader, None);
            context.PSSetShader(&self.pixel_shader, None);
            context.PSSetConstantBuffers(0, Some(&[Some(self.constants.clone())]));
            context.PSSetShaderResources(0, Some(&[Some(view.clone())]));
            context.OMSetRenderTargets(Some(&[Some(target.view.clone())]), None);
            context.RSSetViewports(Some(&[viewport]));
            context.Draw(3, 0);
            // Unbound, so that the caller can write to the source texture again.
            context.PSSetShaderResources(0, Some(&[None]));
            context.OMSetRenderTargets(None, None);
        }
    }

    #[allow(unsafe_code)] // CopyResource, Map and reading the mapped rows
    fn read_back(&self) -> Result<Vec<u8>, Fault> {
        let target = self
            .target
            .as_ref()
            .ok_or_else(|| Fault::Other("no render target".to_owned()))?;
        let context = &self.gpu.context;
        let (width, height) = (usize::from(target.width), usize::from(target.height));
        let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
        // SAFETY: both textures are alive and the same size and format; Map waits for the
        // GPU and fails (leaving the texture unmapped) with a lost device; the out pointer is
        // to a local.
        unsafe {
            context.CopyResource(&target.staging, &target.texture);
            context.Map(&target.staging, 0, D3D11_MAP_READ, 0, Some(&raw mut mapped))?;
        }
        let pitch = mapped.RowPitch as usize;
        let mut rgb = Vec::with_capacity(width * height * 3);
        // SAFETY: a mapped texture is `height` rows of `RowPitch` bytes starting at `pData`,
        // each row holding `width` RGBA pixels, valid until Unmap below; the slice is read
        // before that and not kept.
        unsafe {
            let bytes = std::slice::from_raw_parts(
                mapped.pData.cast::<u8>(),
                pitch * (height - 1) + width * 4,
            );
            for row in 0..height {
                let line = &bytes[row * pitch..row * pitch + width * 4];
                for pixel in line.as_chunks::<4>().0 {
                    rgb.extend_from_slice(&pixel[..3]);
                }
            }
            context.Unmap(&target.staging, 0);
        }
        Ok(rgb)
    }
}

/// The formats the downscaler is tested with and the grabbers use.
pub const BGRA8: DXGI_FORMAT = windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_B8G8R8A8_UNORM;
pub const FP16: DXGI_FORMAT =
    windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_R16G16B16A16_FLOAT;

/// Binds for a texture the downscaler can read.
#[must_use]
pub fn shader_resource_binding() -> u32 {
    D3D11_BIND_SHADER_RESOURCE.0.cast_unsigned()
}
