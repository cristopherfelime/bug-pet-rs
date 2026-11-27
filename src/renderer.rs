use crate::assets::RgbaSprite;
use crate::rig::mesh::RigVertex;
use crate::shaders::{RIG_SKIN_HLSL, SPRITE_HLSL};
use std::mem::size_of;
use windows::core::{s, Interface, PCSTR};
use windows::Win32::Foundation::{BOOL, FALSE, HMODULE, HWND, TRUE};
use windows::Win32::Graphics::Direct3D::Fxc::D3DCompile;
use windows::Win32::Graphics::Direct3D::{
    ID3DBlob, D3D_DRIVER_TYPE_HARDWARE, D3D_DRIVER_TYPE_UNKNOWN, D3D_FEATURE_LEVEL_11_0,
    D3D_FEATURE_LEVEL_11_1, D3D_PRIMITIVE_TOPOLOGY_TRIANGLELIST,
};
use windows::Win32::Graphics::Direct3D11::{
    D3D11CreateDevice, ID3D11BlendState, ID3D11Buffer, ID3D11Device, ID3D11DeviceContext,
    ID3D11InputLayout, ID3D11PixelShader, ID3D11RasterizerState, ID3D11RenderTargetView,
    ID3D11SamplerState, ID3D11ShaderResourceView, ID3D11Texture2D, ID3D11VertexShader,
    D3D11_BIND_CONSTANT_BUFFER, D3D11_BIND_INDEX_BUFFER, D3D11_BIND_RENDER_TARGET,
    D3D11_BIND_SHADER_RESOURCE, D3D11_BIND_VERTEX_BUFFER, D3D11_BLEND_DESC,
    D3D11_BLEND_INV_SRC_ALPHA, D3D11_BLEND_ONE, D3D11_BLEND_OP_ADD, D3D11_BUFFER_DESC,
    D3D11_COLOR_WRITE_ENABLE_ALL, D3D11_COMPARISON_NEVER, D3D11_CPU_ACCESS_WRITE,
    D3D11_CREATE_DEVICE_BGRA_SUPPORT, D3D11_CULL_NONE, D3D11_FILL_SOLID, D3D11_FILTER_ANISOTROPIC,
    D3D11_FLOAT32_MAX, D3D11_INPUT_ELEMENT_DESC, D3D11_INPUT_PER_VERTEX_DATA,
    D3D11_MAPPED_SUBRESOURCE, D3D11_MAP_WRITE_DISCARD, D3D11_RASTERIZER_DESC,
    D3D11_RENDER_TARGET_BLEND_DESC, D3D11_RESOURCE_MISC_GENERATE_MIPS, D3D11_SAMPLER_DESC,
    D3D11_SDK_VERSION, D3D11_SUBRESOURCE_DATA, D3D11_TEXTURE2D_DESC, D3D11_TEXTURE_ADDRESS_CLAMP,
    D3D11_USAGE_DEFAULT, D3D11_USAGE_DYNAMIC, D3D11_USAGE_IMMUTABLE, D3D11_VIEWPORT,
};
use windows::Win32::Graphics::DirectComposition::{
    DCompositionCreateDevice, IDCompositionDevice, IDCompositionTarget, IDCompositionVisual,
};
use windows::Win32::Graphics::Dxgi::Common::{
    DXGI_ALPHA_MODE_PREMULTIPLIED, DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_FORMAT_R32G32B32A32_FLOAT,
    DXGI_FORMAT_R32G32_FLOAT, DXGI_FORMAT_R32_FLOAT, DXGI_FORMAT_R32_UINT,
    DXGI_FORMAT_R8G8B8A8_UNORM, DXGI_SAMPLE_DESC,
};
use windows::Win32::Graphics::Dxgi::{
    CreateDXGIFactory1, IDXGIAdapter1, IDXGIDevice, IDXGIFactory2, IDXGIFactory6, IDXGISwapChain1,
    DXGI_ADAPTER_FLAG_SOFTWARE, DXGI_GPU_PREFERENCE_HIGH_PERFORMANCE,
    DXGI_GPU_PREFERENCE_MINIMUM_POWER, DXGI_PRESENT, DXGI_SCALING_STRETCH,
    DXGI_SWAP_CHAIN_DESC1, DXGI_SWAP_CHAIN_FLAG, DXGI_SWAP_EFFECT_FLIP_SEQUENTIAL,
    DXGI_USAGE_RENDER_TARGET_OUTPUT,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GpuPreference {
    LowPower,
    HighPerformance,
    Auto,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct QuadVertex {
    pos: [f32; 2],
    uv: [f32; 2],
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct SpriteCB {
    pub m0: [f32; 4],
    pub m1: [f32; 4],
    pub vp: [f32; 4],
    pub win_and_pad: [f32; 4],
    pub tint: [f32; 4],
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct RigCB {
    pub parent_m0: [f32; 4],
    pub parent_m1: [f32; 4],
    pub vp: [f32; 4],
    pub win_and_pad: [f32; 4],
    pub mode_and_fade: [f32; 4],
    pub pick_pose: [f32; 4],
    pub dir_flip_len: [f32; 4],
    pub ear_curve: [f32; 4],
    pub ear_root: [f32; 4],
    pub p_d: [[f32; 4]; 13],
    pub joints_rot: [[f32; 4]; 12],
    pub rest_start: [[f32; 4]; 12],
    pub ear_spine: [[f32; 4]; 49],
}

pub struct GpuTexture {
    pub srv: ID3D11ShaderResourceView,
    pub width: u32,
    pub height: u32,
}

pub struct RigGpuMesh {
    vb: ID3D11Buffer,
    ib: ID3D11Buffer,
    index_count: u32,
    tex: GpuTexture,
}

impl GpuTexture {
    pub fn from_sprite(gpu: &GpuContext, sprite: &RgbaSprite) -> Result<Self, String> {
        Self::from_rgba(gpu, sprite.width, sprite.height, &sprite.pixels)
    }

    pub fn from_rgba(
        gpu: &GpuContext,
        width: u32,
        height: u32,
        premul_pixels: &[u8],
    ) -> Result<Self, String> {
        let w = width.max(1);
        let h = height.max(1);
        let tex_desc = D3D11_TEXTURE2D_DESC {
            Width: w,
            Height: h,
            MipLevels: 0,
            ArraySize: 1,
            Format: DXGI_FORMAT_R8G8B8A8_UNORM,
            SampleDesc: DXGI_SAMPLE_DESC {
                Count: 1,
                Quality: 0,
            },
            Usage: D3D11_USAGE_DEFAULT,
            BindFlags: (D3D11_BIND_SHADER_RESOURCE.0 | D3D11_BIND_RENDER_TARGET.0) as u32,
            CPUAccessFlags: 0,
            MiscFlags: D3D11_RESOURCE_MISC_GENERATE_MIPS.0 as u32,
        };

        let mut tex: Option<ID3D11Texture2D> = None;
        unsafe {
            gpu.device
                .CreateTexture2D(&tex_desc, None, Some(&mut tex))
                .map_err(|e| format!("CreateTexture2D({w}x{h}) failed: {e}"))?;
        }
        let tex = tex.ok_or("CreateTexture2D returned null")?;

        unsafe {
            gpu.ctx.UpdateSubresource(
                &tex,
                0,
                None,
                premul_pixels.as_ptr() as *const _,
                w * 4,
                0,
            );
        }

        let mut srv: Option<ID3D11ShaderResourceView> = None;
        unsafe {
            gpu.device
                .CreateShaderResourceView(&tex, None, Some(&mut srv))
                .map_err(|e| format!("CreateShaderResourceView failed: {e}"))?;
        }
        let srv = srv.ok_or("CreateShaderResourceView returned null")?;

        unsafe {
            gpu.ctx.GenerateMips(&srv);
        }

        Ok(Self {
            srv,
            width: w,
            height: h,
        })
    }

    pub fn soft_shadow(gpu: &GpuContext) -> Result<Self, String> {
        const N: u32 = 64;
        let mut pixels = vec![0u8; (N * N * 4) as usize];
        let half = (N as f32 - 1.0) * 0.5;
        for y in 0..N {
            for x in 0..N {
                let dx = (x as f32 - half) / half;
                let dy = (y as f32 - half) / half;
                let r = (dx * dx + dy * dy).sqrt().min(1.0);
                let falloff = (1.0 - r).powf(2.2);
                let v = (falloff * 255.0).round().clamp(0.0, 255.0) as u8;
                let idx = ((y * N + x) * 4) as usize;
                pixels[idx] = v;
                pixels[idx + 1] = v;
                pixels[idx + 2] = v;
                pixels[idx + 3] = v;
            }
        }
        Self::from_rgba(gpu, N, N, &pixels)
    }
}

pub struct GpuContext {
    pub device: ID3D11Device,
    pub ctx: ID3D11DeviceContext,
    pub factory: IDXGIFactory2,
    pub dcomp: IDCompositionDevice,
    pub adapter_name: String,
    vs: ID3D11VertexShader,
    ps: ID3D11PixelShader,
    input_layout: ID3D11InputLayout,
    rig_vs: ID3D11VertexShader,
    rig_ps: ID3D11PixelShader,
    rig_layout: ID3D11InputLayout,
    blend_premul: ID3D11BlendState,
    rasterizer: ID3D11RasterizerState,
    sampler: ID3D11SamplerState,
    quad_vb: ID3D11Buffer,
    sprite_cb: ID3D11Buffer,
    rig_cb: ID3D11Buffer,
    pub shadow_tex: Option<GpuTexture>,
}

fn try_create_device_by_pref(pref: GpuPreference) -> Option<(ID3D11Device, ID3D11DeviceContext)> {
    let dxgi_pref = match pref {
        GpuPreference::LowPower => DXGI_GPU_PREFERENCE_MINIMUM_POWER,
        GpuPreference::HighPerformance => DXGI_GPU_PREFERENCE_HIGH_PERFORMANCE,
        GpuPreference::Auto => return None,
    };
    let factory1: IDXGIFactory6 = unsafe { CreateDXGIFactory1().ok()? };
    let levels = [D3D_FEATURE_LEVEL_11_1, D3D_FEATURE_LEVEL_11_0];

    for idx in 0..8u32 {
        let adapter: IDXGIAdapter1 = match unsafe { factory1.EnumAdapterByGpuPreference(idx, dxgi_pref) } {
            Ok(a) => a,
            Err(_) => break,
        };
        if let Ok(desc) = unsafe { adapter.GetDesc1() } {
            if (desc.Flags & DXGI_ADAPTER_FLAG_SOFTWARE.0 as u32) != 0 {
                continue;
            }
        }
        let mut device: Option<ID3D11Device> = None;
        let mut ctx: Option<ID3D11DeviceContext> = None;
        let res = unsafe {
            D3D11CreateDevice(
                &adapter,
                D3D_DRIVER_TYPE_UNKNOWN,
                HMODULE::default(),
                D3D11_CREATE_DEVICE_BGRA_SUPPORT,
                Some(&levels),
                D3D11_SDK_VERSION,
                Some(&mut device),
                None,
                Some(&mut ctx),
            )
        };
        if res.is_ok() {
            if let (Some(d), Some(c)) = (device, ctx) {
                return Some((d, c));
            }
        }
    }
    None
}

impl GpuContext {
    pub fn new(pref: GpuPreference) -> Result<Self, String> {
        let _keep_type = DXGI_FORMAT_R32G32B32A32_FLOAT;
        let (device, ctx) = if let Some(pair) = try_create_device_by_pref(pref) {
            pair
        } else {
            let mut device: Option<ID3D11Device> = None;
            let mut ctx: Option<ID3D11DeviceContext> = None;
            let levels = [D3D_FEATURE_LEVEL_11_1, D3D_FEATURE_LEVEL_11_0];
            unsafe {
                D3D11CreateDevice(
                    None,
                    D3D_DRIVER_TYPE_HARDWARE,
                    HMODULE::default(),
                    D3D11_CREATE_DEVICE_BGRA_SUPPORT,
                    Some(&levels),
                    D3D11_SDK_VERSION,
                    Some(&mut device),
                    None,
                    Some(&mut ctx),
                )
                .map_err(|e| format!("D3D11CreateDevice failed: {e}"))?;
            }
            (
                device.ok_or("D3D11CreateDevice returned null device")?,
                ctx.ok_or("D3D11CreateDevice returned null context")?,
            )
        };

        let dxgi_device: IDXGIDevice = device
            .cast()
            .map_err(|e| format!("Cast IDXGIDevice failed: {e}"))?;
        let adapter = unsafe {
            dxgi_device
                .GetAdapter()
                .map_err(|e| format!("GetAdapter failed: {e}"))?
        };
        let adapter_desc = unsafe {
            adapter
                .GetDesc()
                .map_err(|e| format!("GetDesc failed: {e}"))?
        };
        let end = adapter_desc
            .Description
            .iter()
            .position(|&c| c == 0)
            .unwrap_or(adapter_desc.Description.len());
        let adapter_name = String::from_utf16_lossy(&adapter_desc.Description[..end]);

        let factory: IDXGIFactory2 = unsafe {
            adapter
                .GetParent()
                .map_err(|e| format!("GetParent(IDXGIFactory2) failed: {e}"))?
        };
        let dcomp: IDCompositionDevice = unsafe {
            DCompositionCreateDevice(&dxgi_device)
                .map_err(|e| format!("DCompositionCreateDevice failed: {e}"))?
        };

        // Sprite shaders
        let vs_blob = compile_hlsl(SPRITE_HLSL, s!("sprite.hlsl"), s!("VSMain"), s!("vs_5_0"))?;
        let ps_blob = compile_hlsl(SPRITE_HLSL, s!("sprite.hlsl"), s!("PSMain"), s!("ps_5_0"))?;
        let vs_bytes = blob_bytes(&vs_blob);
        let ps_bytes = blob_bytes(&ps_blob);

        let mut vs: Option<ID3D11VertexShader> = None;
        let mut ps: Option<ID3D11PixelShader> = None;
        unsafe {
            device.CreateVertexShader(vs_bytes, None, Some(&mut vs)).map_err(|e| format!("CreateVS failed: {e}"))?;
            device.CreatePixelShader(ps_bytes, None, Some(&mut ps)).map_err(|e| format!("CreatePS failed: {e}"))?;
        }
        let vs = vs.ok_or("CreateVS null")?;
        let ps = ps.ok_or("CreatePS null")?;

        let sprite_layout_desc = [
            D3D11_INPUT_ELEMENT_DESC {
                SemanticName: s!("POSITION"),
                SemanticIndex: 0,
                Format: DXGI_FORMAT_R32G32_FLOAT,
                InputSlot: 0,
                AlignedByteOffset: 0,
                InputSlotClass: D3D11_INPUT_PER_VERTEX_DATA,
                InstanceDataStepRate: 0,
            },
            D3D11_INPUT_ELEMENT_DESC {
                SemanticName: s!("TEXCOORD"),
                SemanticIndex: 0,
                Format: DXGI_FORMAT_R32G32_FLOAT,
                InputSlot: 0,
                AlignedByteOffset: 8,
                InputSlotClass: D3D11_INPUT_PER_VERTEX_DATA,
                InstanceDataStepRate: 0,
            },
        ];
        let mut input_layout: Option<ID3D11InputLayout> = None;
        unsafe {
            device
                .CreateInputLayout(&sprite_layout_desc, vs_bytes, Some(&mut input_layout))
                .map_err(|e| format!("CreateInputLayout(sprite) failed: {e}"))?;
        }
        let input_layout = input_layout.ok_or("CreateInputLayout(sprite) null")?;

        // Rig skinning shaders
        let rvs_blob = compile_hlsl(RIG_SKIN_HLSL, s!("rig_skin.hlsl"), s!("VSMain"), s!("vs_5_0"))?;
        let rps_blob = compile_hlsl(RIG_SKIN_HLSL, s!("rig_skin.hlsl"), s!("PSMain"), s!("ps_5_0"))?;
        let rvs_bytes = blob_bytes(&rvs_blob);
        let rps_bytes = blob_bytes(&rps_blob);

        let mut rig_vs: Option<ID3D11VertexShader> = None;
        let mut rig_ps: Option<ID3D11PixelShader> = None;
        unsafe {
            device.CreateVertexShader(rvs_bytes, None, Some(&mut rig_vs)).map_err(|e| format!("CreateRigVS failed: {e}"))?;
            device.CreatePixelShader(rps_bytes, None, Some(&mut rig_ps)).map_err(|e| format!("CreateRigPS failed: {e}"))?;
        }
        let rig_vs = rig_vs.ok_or("CreateRigVS null")?;
        let rig_ps = rig_ps.ok_or("CreateRigPS null")?;

        let rig_layout_desc = [
            D3D11_INPUT_ELEMENT_DESC {
                SemanticName: s!("POSITION"),
                SemanticIndex: 0,
                Format: DXGI_FORMAT_R32G32_FLOAT,
                InputSlot: 0,
                AlignedByteOffset: 0,
                InputSlotClass: D3D11_INPUT_PER_VERTEX_DATA,
                InstanceDataStepRate: 0,
            },
            D3D11_INPUT_ELEMENT_DESC {
                SemanticName: s!("TEXCOORD"),
                SemanticIndex: 0,
                Format: DXGI_FORMAT_R32G32_FLOAT,
                InputSlot: 0,
                AlignedByteOffset: 8,
                InputSlotClass: D3D11_INPUT_PER_VERTEX_DATA,
                InstanceDataStepRate: 0,
            },
            D3D11_INPUT_ELEMENT_DESC {
                SemanticName: s!("TEXCOORD"),
                SemanticIndex: 1,
                Format: DXGI_FORMAT_R32_FLOAT,
                InputSlot: 0,
                AlignedByteOffset: 16,
                InputSlotClass: D3D11_INPUT_PER_VERTEX_DATA,
                InstanceDataStepRate: 0,
            },
        ];
        let mut rig_layout: Option<ID3D11InputLayout> = None;
        unsafe {
            device
                .CreateInputLayout(&rig_layout_desc, rvs_bytes, Some(&mut rig_layout))
                .map_err(|e| format!("CreateInputLayout(rig) failed: {e}"))?;
        }
        let rig_layout = rig_layout.ok_or("CreateInputLayout(rig) null")?;

        let mut blend_desc = D3D11_BLEND_DESC::default();
        blend_desc.RenderTarget[0] = D3D11_RENDER_TARGET_BLEND_DESC {
            BlendEnable: TRUE,
            SrcBlend: D3D11_BLEND_ONE,
            DestBlend: D3D11_BLEND_INV_SRC_ALPHA,
            BlendOp: D3D11_BLEND_OP_ADD,
            SrcBlendAlpha: D3D11_BLEND_ONE,
            DestBlendAlpha: D3D11_BLEND_INV_SRC_ALPHA,
            BlendOpAlpha: D3D11_BLEND_OP_ADD,
            RenderTargetWriteMask: D3D11_COLOR_WRITE_ENABLE_ALL.0 as u8,
        };
        let mut blend_premul: Option<ID3D11BlendState> = None;
        unsafe {
            device
                .CreateBlendState(&blend_desc, Some(&mut blend_premul))
                .map_err(|e| format!("CreateBlendState failed: {e}"))?;
        }
        let blend_premul = blend_premul.ok_or("CreateBlendState returned null")?;

        let rast_desc = D3D11_RASTERIZER_DESC {
            FillMode: D3D11_FILL_SOLID,
            CullMode: D3D11_CULL_NONE,
            FrontCounterClockwise: FALSE,
            DepthBias: 0,
            DepthBiasClamp: 0.0,
            SlopeScaledDepthBias: 0.0,
            DepthClipEnable: TRUE,
            ScissorEnable: FALSE,
            MultisampleEnable: FALSE,
            AntialiasedLineEnable: FALSE,
        };
        let mut rasterizer: Option<ID3D11RasterizerState> = None;
        unsafe {
            device
                .CreateRasterizerState(&rast_desc, Some(&mut rasterizer))
                .map_err(|e| format!("CreateRasterizerState failed: {e}"))?;
        }
        let rasterizer = rasterizer.ok_or("CreateRasterizerState returned null")?;

        let samp_desc = D3D11_SAMPLER_DESC {
            Filter: D3D11_FILTER_ANISOTROPIC,
            AddressU: D3D11_TEXTURE_ADDRESS_CLAMP,
            AddressV: D3D11_TEXTURE_ADDRESS_CLAMP,
            AddressW: D3D11_TEXTURE_ADDRESS_CLAMP,
            MipLODBias: 0.0,
            MaxAnisotropy: 8,
            ComparisonFunc: D3D11_COMPARISON_NEVER,
            BorderColor: [0.0; 4],
            MinLOD: 0.0,
            MaxLOD: D3D11_FLOAT32_MAX,
        };
        let mut sampler: Option<ID3D11SamplerState> = None;
        unsafe {
            device
                .CreateSamplerState(&samp_desc, Some(&mut sampler))
                .map_err(|e| format!("CreateSamplerState failed: {e}"))?;
        }
        let sampler = sampler.ok_or("CreateSamplerState returned null")?;

        let quad_verts = [
            QuadVertex { pos: [0.0, 0.0], uv: [0.0, 0.0] },
            QuadVertex { pos: [1.0, 0.0], uv: [1.0, 0.0] },
            QuadVertex { pos: [0.0, 1.0], uv: [0.0, 1.0] },
            QuadVertex { pos: [1.0, 0.0], uv: [1.0, 0.0] },
            QuadVertex { pos: [1.0, 1.0], uv: [1.0, 1.0] },
            QuadVertex { pos: [0.0, 1.0], uv: [0.0, 1.0] },
        ];
        let vb_desc = D3D11_BUFFER_DESC {
            ByteWidth: (quad_verts.len() * size_of::<QuadVertex>()) as u32,
            Usage: D3D11_USAGE_IMMUTABLE,
            BindFlags: D3D11_BIND_VERTEX_BUFFER.0 as u32,
            CPUAccessFlags: 0,
            MiscFlags: 0,
            StructureByteStride: 0,
        };
        let vb_data = D3D11_SUBRESOURCE_DATA {
            pSysMem: quad_verts.as_ptr() as *const _,
            SysMemPitch: 0,
            SysMemSlicePitch: 0,
        };
        let mut quad_vb: Option<ID3D11Buffer> = None;
        unsafe {
            device
                .CreateBuffer(&vb_desc, Some(&vb_data), Some(&mut quad_vb))
                .map_err(|e| format!("CreateBuffer(quad_vb) failed: {e}"))?;
        }
        let quad_vb = quad_vb.ok_or("CreateBuffer(quad_vb) returned null")?;

        let sprite_cb = create_dynamic_cb(&device, size_of::<SpriteCB>() as u32)?;
        let rig_cb = create_dynamic_cb(&device, size_of::<RigCB>() as u32)?;

        let mut this = Self {
            device,
            ctx,
            factory,
            dcomp,
            adapter_name,
            vs,
            ps,
            input_layout,
            rig_vs,
            rig_ps,
            rig_layout,
            blend_premul,
            rasterizer,
            sampler,
            quad_vb,
            sprite_cb,
            rig_cb,
            shadow_tex: None,
        };
        this.shadow_tex = Some(GpuTexture::soft_shadow(&this)?);
        Ok(this)
    }

    pub fn create_rig_mesh(
        &self,
        verts: &[RigVertex],
        indices: &[u32],
        sprite: &RgbaSprite,
    ) -> Result<RigGpuMesh, String> {
        let vb_desc = D3D11_BUFFER_DESC {
            ByteWidth: (verts.len() * size_of::<RigVertex>()) as u32,
            Usage: D3D11_USAGE_IMMUTABLE,
            BindFlags: D3D11_BIND_VERTEX_BUFFER.0 as u32,
            CPUAccessFlags: 0,
            MiscFlags: 0,
            StructureByteStride: 0,
        };
        let vb_data = D3D11_SUBRESOURCE_DATA {
            pSysMem: verts.as_ptr() as *const _,
            SysMemPitch: 0,
            SysMemSlicePitch: 0,
        };
        let mut vb: Option<ID3D11Buffer> = None;
        unsafe {
            self.device
                .CreateBuffer(&vb_desc, Some(&vb_data), Some(&mut vb))
                .map_err(|e| format!("CreateBuffer(rig_vb) failed: {e}"))?;
        }
        let vb = vb.ok_or("CreateBuffer(rig_vb) null")?;

        let ib_desc = D3D11_BUFFER_DESC {
            ByteWidth: (indices.len() * size_of::<u32>()) as u32,
            Usage: D3D11_USAGE_IMMUTABLE,
            BindFlags: D3D11_BIND_INDEX_BUFFER.0 as u32,
            CPUAccessFlags: 0,
            MiscFlags: 0,
            StructureByteStride: 0,
        };
        let ib_data = D3D11_SUBRESOURCE_DATA {
            pSysMem: indices.as_ptr() as *const _,
            SysMemPitch: 0,
            SysMemSlicePitch: 0,
        };
        let mut ib: Option<ID3D11Buffer> = None;
        unsafe {
            self.device
                .CreateBuffer(&ib_desc, Some(&ib_data), Some(&mut ib))
                .map_err(|e| format!("CreateBuffer(rig_ib) failed: {e}"))?;
        }
        let ib = ib.ok_or("CreateBuffer(rig_ib) null")?;

        let tex = GpuTexture::from_sprite(self, sprite)?;
        Ok(RigGpuMesh {
            vb,
            ib,
            index_count: indices.len() as u32,
            tex,
        })
    }

    pub fn commit(&self) -> Result<(), String> {
        unsafe {
            self.dcomp
                .Commit()
                .map_err(|e| format!("DCompositionCommit failed: {e}"))
        }
    }
}

fn create_dynamic_cb(device: &ID3D11Device, byte_width: u32) -> Result<ID3D11Buffer, String> {
    let cb_desc = D3D11_BUFFER_DESC {
        ByteWidth: byte_width,
        Usage: D3D11_USAGE_DYNAMIC,
        BindFlags: D3D11_BIND_CONSTANT_BUFFER.0 as u32,
        CPUAccessFlags: D3D11_CPU_ACCESS_WRITE.0 as u32,
        MiscFlags: 0,
        StructureByteStride: 0,
    };
    let mut cb: Option<ID3D11Buffer> = None;
    unsafe {
        device
            .CreateBuffer(&cb_desc, None, Some(&mut cb))
            .map_err(|e| format!("CreateBuffer(cb {byte_width}B) failed: {e}"))?;
    }
    cb.ok_or_else(|| "CreateBuffer(cb) returned null".to_string())
}

pub struct PetSurface {
    swap_chain: IDXGISwapChain1,
    _target: IDCompositionTarget,
    _visual: IDCompositionVisual,
    rtv: Option<ID3D11RenderTargetView>,
    pub width_px: u32,
    pub height_px: u32,
}

impl PetSurface {
    pub fn new(
        gpu: &GpuContext,
        hwnd: HWND,
        width_px: u32,
        height_px: u32,
    ) -> Result<Self, String> {
        let w = width_px.max(1);
        let h = height_px.max(1);
        let sc_desc = DXGI_SWAP_CHAIN_DESC1 {
            Width: w,
            Height: h,
            Format: DXGI_FORMAT_B8G8R8A8_UNORM,
            Stereo: BOOL(0),
            SampleDesc: DXGI_SAMPLE_DESC {
                Count: 1,
                Quality: 0,
            },
            BufferUsage: DXGI_USAGE_RENDER_TARGET_OUTPUT,
            BufferCount: 2,
            Scaling: DXGI_SCALING_STRETCH,
            SwapEffect: DXGI_SWAP_EFFECT_FLIP_SEQUENTIAL,
            AlphaMode: DXGI_ALPHA_MODE_PREMULTIPLIED,
            Flags: 0,
        };

        let swap_chain = unsafe {
            gpu.factory
                .CreateSwapChainForComposition(&gpu.device, &sc_desc, None)
                .map_err(|e| format!("CreateSwapChainForComposition({w}x{h}) failed: {e}"))?
        };
        let target = unsafe {
            gpu.dcomp
                .CreateTargetForHwnd(hwnd, true)
                .map_err(|e| format!("CreateTargetForHwnd failed: {e}"))?
        };
        let visual = unsafe {
            gpu.dcomp
                .CreateVisual()
                .map_err(|e| format!("CreateVisual failed: {e}"))?
        };
        unsafe {
            visual
                .SetContent(&swap_chain)
                .map_err(|e| format!("Visual::SetContent failed: {e}"))?;
            target
                .SetRoot(&visual)
                .map_err(|e| format!("Target::SetRoot failed: {e}"))?;
        }

        let mut surface = Self {
            swap_chain,
            _target: target,
            _visual: visual,
            rtv: None,
            width_px: w,
            height_px: h,
        };
        surface.create_rtv(gpu)?;
        Ok(surface)
    }

    fn create_rtv(&mut self, gpu: &GpuContext) -> Result<(), String> {
        let backbuf: ID3D11Texture2D = unsafe {
            self.swap_chain
                .GetBuffer(0)
                .map_err(|e| format!("SwapChain::GetBuffer failed: {e}"))?
        };
        let mut rtv: Option<ID3D11RenderTargetView> = None;
        unsafe {
            gpu.device
                .CreateRenderTargetView(&backbuf, None, Some(&mut rtv))
                .map_err(|e| format!("CreateRenderTargetView failed: {e}"))?;
        }
        self.rtv = Some(rtv.ok_or("CreateRenderTargetView returned null")?);
        Ok(())
    }

    #[allow(dead_code)]
    pub fn resize(&mut self, gpu: &GpuContext, width_px: u32, height_px: u32) -> Result<(), String> {
        let w = width_px.max(1);
        let h = height_px.max(1);
        if w == self.width_px && h == self.height_px {
            return Ok(());
        }
        self.rtv = None;
        unsafe {
            gpu.ctx.OMSetRenderTargets(None, None);
            self.swap_chain
                .ResizeBuffers(2, w, h, DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_SWAP_CHAIN_FLAG(0))
                .map_err(|e| format!("ResizeBuffers({w}x{h}) failed: {e}"))?;
        }
        self.width_px = w;
        self.height_px = h;
        self.create_rtv(gpu)
    }

    pub fn begin_frame(&self, gpu: &GpuContext) {
        self.begin_frame_with_clear(gpu, [0.0f32, 0.0, 0.0, 0.0]);
    }

    pub fn begin_frame_with_clear(&self, gpu: &GpuContext, clear_color: [f32; 4]) {
        let Some(ref rtv) = self.rtv else { return };
        let viewport = D3D11_VIEWPORT {
            TopLeftX: 0.0,
            TopLeftY: 0.0,
            Width: self.width_px as f32,
            Height: self.height_px as f32,
            MinDepth: 0.0,
            MaxDepth: 1.0,
        };
        unsafe {
            gpu.ctx
                .OMSetRenderTargets(Some(&[Some(rtv.clone())]), None);
            gpu.ctx.ClearRenderTargetView(rtv, &clear_color);
            gpu.ctx.RSSetViewports(Some(&[viewport]));
            gpu.ctx.RSSetState(&gpu.rasterizer);
            gpu.ctx
                .OMSetBlendState(&gpu.blend_premul, Some(&[0.0; 4]), 0xffff_ffff);
            gpu.ctx
                .IASetPrimitiveTopology(D3D_PRIMITIVE_TOPOLOGY_TRIANGLELIST);
            gpu.ctx.PSSetSamplers(0, Some(&[Some(gpu.sampler.clone())]));
        }
    }

    pub fn draw_sprite(
        &self,
        gpu: &GpuContext,
        tex: &GpuTexture,
        cb_data: &SpriteCB,
    ) -> Result<(), String> {
        let stride = size_of::<QuadVertex>() as u32;
        let offset = 0u32;
        let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
        unsafe {
            gpu.ctx
                .Map(&gpu.sprite_cb, 0, D3D11_MAP_WRITE_DISCARD, 0, Some(&mut mapped))
                .map_err(|e| format!("Map(sprite_cb) failed: {e}"))?;
            std::ptr::copy_nonoverlapping(
                cb_data as *const SpriteCB,
                mapped.pData as *mut SpriteCB,
                1,
            );
            gpu.ctx.Unmap(&gpu.sprite_cb, 0);

            gpu.ctx.IASetInputLayout(&gpu.input_layout);
            gpu.ctx.IASetVertexBuffers(0, 1, Some(&Some(gpu.quad_vb.clone())), Some(&stride), Some(&offset));
            gpu.ctx.VSSetShader(&gpu.vs, None);
            gpu.ctx.PSSetShader(&gpu.ps, None);
            gpu.ctx.VSSetConstantBuffers(0, Some(&[Some(gpu.sprite_cb.clone())]));
            gpu.ctx.PSSetConstantBuffers(0, Some(&[Some(gpu.sprite_cb.clone())]));
            gpu.ctx.PSSetShaderResources(0, Some(&[Some(tex.srv.clone())]));
            gpu.ctx.Draw(6, 0);
        }
        Ok(())
    }

    pub fn draw_rig_mesh(
        &self,
        gpu: &GpuContext,
        mesh: &RigGpuMesh,
        cb_data: &RigCB,
    ) -> Result<(), String> {
        let stride = size_of::<RigVertex>() as u32;
        let offset = 0u32;
        let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
        unsafe {
            gpu.ctx
                .Map(&gpu.rig_cb, 0, D3D11_MAP_WRITE_DISCARD, 0, Some(&mut mapped))
                .map_err(|e| format!("Map(rig_cb) failed: {e}"))?;
            std::ptr::copy_nonoverlapping(
                cb_data as *const RigCB,
                mapped.pData as *mut RigCB,
                1,
            );
            gpu.ctx.Unmap(&gpu.rig_cb, 0);

            gpu.ctx.IASetInputLayout(&gpu.rig_layout);
            gpu.ctx.IASetVertexBuffers(0, 1, Some(&Some(mesh.vb.clone())), Some(&stride), Some(&offset));
            gpu.ctx.IASetIndexBuffer(&mesh.ib, DXGI_FORMAT_R32_UINT, 0);
            gpu.ctx.VSSetShader(&gpu.rig_vs, None);
            gpu.ctx.PSSetShader(&gpu.rig_ps, None);
            gpu.ctx.VSSetConstantBuffers(0, Some(&[Some(gpu.rig_cb.clone())]));
            gpu.ctx.PSSetConstantBuffers(0, Some(&[Some(gpu.rig_cb.clone())]));
            gpu.ctx.PSSetShaderResources(0, Some(&[Some(mesh.tex.srv.clone())]));
            gpu.ctx.DrawIndexed(mesh.index_count, 0, 0);
        }
        Ok(())
    }

    pub fn present(&self) {
        unsafe {
            let _ = self.swap_chain.Present(0, DXGI_PRESENT(0));
        }
    }
}

fn compile_hlsl(source: &str, name: PCSTR, entry: PCSTR, target: PCSTR) -> Result<ID3DBlob, String> {
    let mut blob: Option<ID3DBlob> = None;
    let mut err: Option<ID3DBlob> = None;
    let res = unsafe {
        D3DCompile(
            source.as_ptr() as *const _,
            source.len(),
            name,
            None,
            None,
            entry,
            target,
            0,
            0,
            &mut blob,
            Some(&mut err),
        )
    };
    if let Err(e) = res {
        let msg = err
            .map(|b| String::from_utf8_lossy(blob_bytes(&b)).into_owned())
            .unwrap_or_else(|| e.to_string());
        return Err(format!("HLSL compile error: {msg}"));
    }
    blob.ok_or_else(|| "D3DCompile succeeded with null blob".to_string())
}

fn blob_bytes(blob: &ID3DBlob) -> &[u8] {
    unsafe {
        std::slice::from_raw_parts(blob.GetBufferPointer() as *const u8, blob.GetBufferSize())
    }
}
