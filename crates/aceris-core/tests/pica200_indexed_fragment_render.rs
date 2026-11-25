//! End-to-end regressions for indexed PICA200 draws through the fragment backend.

use aceris_core::gpu::{
    ClipDepthRange, FCRAM_BASE, GpuPhysicalMemory, Pica200, PicaFragmentRenderSession,
    PicaRenderSettings, PicaRenderTargetConfig, REG_ATTRIBBUFFER0_OFFSET,
    REG_ATTRIBBUFFERS_FORMAT_HIGH, REG_ATTRIBBUFFERS_FORMAT_LOW, REG_ATTRIBBUFFERS_LOC,
    REG_COLOR_OPERATION, REG_COLORBUFFER_FORMAT, REG_COLORBUFFER_LOC, REG_COLORBUFFER_WRITE,
    REG_DEPTH_COLOR_MASK, REG_DEPTHMAP_ENABLE, REG_DEPTHMAP_OFFSET, REG_DEPTHMAP_SCALE,
    REG_FRAMEBUFFER_BLOCK32, REG_FRAMEBUFFER_DIM, REG_INDEXBUFFER_CONFIG, REG_LOGIC_OP,
    REG_NUMVERTICES, REG_SH_OUTMAP_O0, REG_SH_OUTMAP_TOTAL, REG_TEXUNIT_CONFIG,
    REG_VSH_ATTRIBUTES_PERMUTATION_HIGH, REG_VSH_ATTRIBUTES_PERMUTATION_LOW,
    REG_VSH_CODETRANSFER_DATA0, REG_VSH_CODETRANSFER_INDEX, REG_VSH_ENTRYPOINT,
    REG_VSH_FLOATUNIFORM_DATA0, REG_VSH_FLOATUNIFORM_INDEX, REG_VSH_INPUTBUFFER_CONFIG,
    REG_VSH_OPDESCS_DATA0, REG_VSH_OPDESCS_INDEX, REG_VSH_OUTMAP_MASK,
    TEXENV_STAGE_SOURCE_REGISTERS,
};

const VERTEX_BASE: u32 = FCRAM_BASE + 0x1000;
const INDEX_OFFSET: u32 = 0x10;
const COLOR_TARGET: u32 = FCRAM_BASE + 0x4000;
const TARGET_WIDTH: u32 = 8;
const TARGET_HEIGHT: u32 = 8;
const DRAW_ELEMENTS_LIST: [u32; 2] = [1, 0x000F_022F];

fn identity_descriptor(destination_mask: u32) -> u32 {
    destination_mask | (0x1B << 5) | (0x1B << 14) | (0x1B << 23)
}

fn dphi(uniform: u32, descriptor: u32) -> u32 {
    (0x18 << 26) | ((0x20 + uniform) << 7) | descriptor
}

/// Programs a guest vertex shader that transforms byte XY positions into clip space.
fn configure_vertex_stage(gpu: &mut Pica200) {
    for (register, value) in [
        (REG_ATTRIBBUFFERS_LOC, VERTEX_BASE >> 3),
        (REG_ATTRIBBUFFERS_FORMAT_LOW, 1),
        (REG_ATTRIBBUFFERS_FORMAT_HIGH, 0),
        (REG_ATTRIBBUFFER0_OFFSET, 0),
        (REG_ATTRIBBUFFER0_OFFSET + 1, 0),
        (REG_ATTRIBBUFFER0_OFFSET + 2, (2 << 16) | (2 << 28)),
        (REG_VSH_INPUTBUFFER_CONFIG, 0),
        (REG_VSH_ATTRIBUTES_PERMUTATION_LOW, 0),
        (REG_VSH_ATTRIBUTES_PERMUTATION_HIGH, 0),
        (REG_VSH_ENTRYPOINT, 0),
    ] {
        gpu.write_register(register, value).unwrap();
    }

    gpu.write_register(REG_VSH_OPDESCS_INDEX, 0).unwrap();
    for lane in 0..4 {
        gpu.write_register(REG_VSH_OPDESCS_DATA0, identity_descriptor(1 << (3 - lane)))
            .unwrap();
    }

    gpu.write_register(REG_VSH_CODETRANSFER_INDEX, 0).unwrap();
    for uniform in 0..4 {
        gpu.write_register(REG_VSH_CODETRANSFER_DATA0, dphi(uniform, uniform))
            .unwrap();
    }
    gpu.write_register(REG_VSH_CODETRANSFER_DATA0, 0x22 << 26)
        .unwrap();

    // The four DPHI instructions multiply v0 by a guest-uploaded matrix.
    gpu.write_register(REG_VSH_FLOATUNIFORM_INDEX, 0x8000_0000)
        .unwrap();
    for vector in [
        [0.25f32, 0.0, 0.0, -1.0],
        [0.0, 0.25, 0.0, -1.0],
        [0.0, 0.0, 1.0, 0.0],
        [0.0, 0.0, 0.0, 1.0],
    ] {
        for component in vector.into_iter().rev() {
            gpu.write_register(REG_VSH_FLOATUNIFORM_DATA0, component.to_bits())
                .unwrap();
        }
    }

    for index in 0..7u16 {
        gpu.write_register(REG_SH_OUTMAP_O0 + index, 0x1F1F_1F1F)
            .unwrap();
    }
    gpu.write_register(REG_SH_OUTMAP_O0, 0x0302_0100).unwrap();
    gpu.write_register(REG_SH_OUTMAP_TOTAL, 1).unwrap();
    gpu.write_register(REG_VSH_OUTMAP_MASK, 1).unwrap();
}

fn configure_fragment_stage(gpu: &mut Pica200) {
    for (stage, &base) in TEXENV_STAGE_SOURCE_REGISTERS.iter().enumerate() {
        let source = if stage == 0 { 0 } else { 15 };
        gpu.write_register(base, source | (source << 16)).unwrap();
        gpu.write_register(base + 1, 0).unwrap();
        gpu.write_register(base + 2, 0).unwrap();
        gpu.write_register(base + 3, 0).unwrap();
        gpu.write_register(base + 4, 0).unwrap();
    }

    for (register, value) in [
        (REG_TEXUNIT_CONFIG, 0),
        (REG_COLOR_OPERATION, 0),
        (REG_LOGIC_OP, 3),
        (REG_COLORBUFFER_LOC, COLOR_TARGET >> 3),
        (REG_COLORBUFFER_FORMAT, 2),
        (REG_COLORBUFFER_WRITE, 0xF),
        (REG_DEPTH_COLOR_MASK, 0xF00),
        (REG_FRAMEBUFFER_BLOCK32, 0),
        (
            REG_FRAMEBUFFER_DIM,
            ((TARGET_HEIGHT - 1) << 12) | TARGET_WIDTH | (1 << 24),
        ),
        (REG_DEPTHMAP_SCALE, 0x003E_0000),
        (REG_DEPTHMAP_OFFSET, 0x003E_0000),
        (REG_DEPTHMAP_ENABLE, 1),
    ] {
        gpu.write_register(register, value).unwrap();
    }

    // A 4-pixel half-size viewport maps the 8x8 surface.
    gpu.write_register(0x041, 0x0041_0000).unwrap();
    gpu.write_register(0x043, 0x0041_0000).unwrap();
    gpu.write_register(0x068, 0).unwrap();
}

#[test]
fn draw_elements_runs_guest_vertex_shader_and_fragment_pipeline_into_tiled_target() {
    let mut gpu = Pica200::new();
    configure_vertex_stage(&mut gpu);
    configure_fragment_stage(&mut gpu);

    gpu.write_register(REG_INDEXBUFFER_CONFIG, 0x8000_0000 | INDEX_OFFSET)
        .unwrap();
    gpu.write_register(REG_NUMVERTICES, 3).unwrap();

    let mut memory = GpuPhysicalMemory::new();
    // The guest's vertex loader reads four packed unsigned-byte XY pairs.
    memory
        .write(VERTEX_BASE, &[0, 0, 8, 0, 0, 8, 8, 8])
        .unwrap();
    // U16 indices [3, 2, 1] select the upper-right triangle, not [0, 1, 2].
    memory
        .write(VERTEX_BASE + INDEX_OFFSET, &[3, 0, 2, 0, 1, 0])
        .unwrap();

    let mut session = PicaFragmentRenderSession::new(PicaRenderSettings {
        clip_depth_range: ClipDepthRange::NegativeOneToOne,
        default_color: [200, 100, 50, 255],
    });
    let (command_report, render_report) = session
        .execute_command_list(&mut gpu, &DRAW_ELEMENTS_LIST, &mut memory, |_, _| {
            Some(VERTEX_BASE)
        })
        .unwrap();

    assert_eq!(command_report.commands, 1);
    assert_eq!(render_report.triangles_submitted, 1);
    assert_eq!(render_report.executed_instructions, 15);
    assert!(render_report.pixels_written >= 28, "{render_report:?}");

    session.flush(&mut memory).unwrap();
    let target = PicaRenderTargetConfig::from_registers(
        COLOR_TARGET >> 3,
        2,
        0,
        ((TARGET_HEIGHT - 1) << 12) | TARGET_WIDTH | (1 << 24),
    )
    .unwrap();
    let pixels = target.read_rgba8(&memory).unwrap();
    let pixel_at = |x: usize, y: usize| {
        let offset = (y * TARGET_WIDTH as usize + x) * 4;
        [
            pixels[offset],
            pixels[offset + 1],
            pixels[offset + 2],
            pixels[offset + 3],
        ]
    };

    assert_eq!(pixel_at(6, 1), [200, 100, 50, 255]);
    assert_eq!(pixel_at(1, 6), [0, 0, 0, 0]);
}
