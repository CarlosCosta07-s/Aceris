//! Generates the Aceris icon in every format the project needs.
//!
//! Run from anywhere inside the repository:
//!
//! ```sh
//! cargo run --release -p aceris-icongen
//! ```
//!
//! Outputs (relative to the repository root, or to the directory given as the
//! first argument):
//!
//! - `assets/icon/aceris.svg` and `aceris-foreground.svg`
//! - `assets/icon/png/aceris-<size>.png`
//! - `assets/icon/aceris.ico`
//! - `frontends/android/res/` launcher icons for every density

mod ico;
mod png;
mod raster;
mod scene;

use scene::{ADAPTIVE_BACKGROUND, Variant, icon};
use std::{
    env, fs, io,
    path::{Path, PathBuf},
    process::ExitCode,
};

/// Sizes of the desktop and store PNGs.
const DESKTOP_SIZES: [u32; 8] = [16, 32, 48, 64, 128, 256, 512, 1024];

/// Sizes packed into the Windows `.ico`.
const ICO_SIZES: [u32; 6] = [16, 32, 48, 64, 128, 256];

/// Android density buckets and their scale relative to `mdpi`, in quarters.
const ANDROID_DENSITIES: [(&str, u32); 5] = [
    ("mdpi", 4),
    ("hdpi", 6),
    ("xhdpi", 8),
    ("xxhdpi", 12),
    ("xxxhdpi", 16),
];

/// Launcher icon size at `mdpi`, in pixels.
const LAUNCHER_DP: u32 = 48;

/// Adaptive-icon layer size at `mdpi`, in pixels.
const ADAPTIVE_DP: u32 = 108;

fn scaled(dp: u32, quarters: u32) -> u32 {
    dp * quarters / 4
}

fn render_png(variant: Variant, size: u32) -> Vec<u8> {
    let image = raster::render(&icon(variant), size);
    png::encode_rgba(image.width, image.height, &image.rgba)
}

fn write(path: &Path, contents: impl AsRef<[u8]>) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(path, contents)?;
    println!("wrote {}", path.display());
    Ok(())
}

fn adaptive_icon_xml() -> &'static str {
    "<?xml version=\"1.0\" encoding=\"utf-8\"?>\n\
     <adaptive-icon xmlns:android=\"http://schemas.android.com/apk/res/android\">\n    \
     <background android:drawable=\"@color/ic_launcher_background\"/>\n    \
     <foreground android:drawable=\"@mipmap/ic_launcher_foreground\"/>\n\
     </adaptive-icon>\n"
}

fn background_color_xml() -> String {
    let scene::Rgb(r, g, b) = ADAPTIVE_BACKGROUND;
    format!(
        "<?xml version=\"1.0\" encoding=\"utf-8\"?>\n<resources>\n    \
         <color name=\"ic_launcher_background\">#{r:02X}{g:02X}{b:02X}</color>\n</resources>\n"
    )
}

fn generate(root: &Path) -> io::Result<()> {
    let icons = root.join("assets").join("icon");
    write(&icons.join("aceris.svg"), icon(Variant::Rounded).to_svg())?;
    write(
        &icons.join("aceris-foreground.svg"),
        icon(Variant::Foreground).to_svg(),
    )?;

    let mut ico_entries = Vec::new();
    for size in DESKTOP_SIZES {
        let png = render_png(Variant::Rounded, size);
        write(&icons.join("png").join(format!("aceris-{size}.png")), &png)?;
        if ICO_SIZES.contains(&size) {
            ico_entries.push(ico::Entry { size, png });
        }
    }
    write(&icons.join("aceris.ico"), ico::encode(&ico_entries))?;

    let res = root.join("frontends").join("android").join("res");
    for (density, quarters) in ANDROID_DENSITIES {
        let folder = res.join(format!("mipmap-{density}"));
        let launcher = scaled(LAUNCHER_DP, quarters);
        let layer = scaled(ADAPTIVE_DP, quarters);
        write(
            &folder.join("ic_launcher.png"),
            render_png(Variant::Rounded, launcher),
        )?;
        write(
            &folder.join("ic_launcher_round.png"),
            render_png(Variant::Circle, launcher),
        )?;
        write(
            &folder.join("ic_launcher_foreground.png"),
            render_png(Variant::Foreground, layer),
        )?;
    }

    let adaptive = res.join("mipmap-anydpi-v26");
    write(&adaptive.join("ic_launcher.xml"), adaptive_icon_xml())?;
    write(&adaptive.join("ic_launcher_round.xml"), adaptive_icon_xml())?;
    write(
        &res.join("values").join("ic_launcher_background.xml"),
        background_color_xml(),
    )
}

/// The repository root: the first argument if given, otherwise two levels
/// above this crate (`tools/icongen`).
fn repository_root() -> PathBuf {
    env::args_os().nth(1).map(PathBuf::from).unwrap_or_else(|| {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .ancestors()
            .nth(2)
            .map_or_else(|| PathBuf::from("."), Path::to_path_buf)
    })
}

fn main() -> ExitCode {
    match generate(&repository_root()) {
        Ok(()) => {
            println!("icons generated");
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("error: could not generate icons: {error}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn android_sizes_follow_the_density_buckets() {
        let launcher: Vec<u32> = ANDROID_DENSITIES
            .iter()
            .map(|&(_, quarters)| scaled(LAUNCHER_DP, quarters))
            .collect();
        assert_eq!(launcher, [48, 72, 96, 144, 192]);

        let layers: Vec<u32> = ANDROID_DENSITIES
            .iter()
            .map(|&(_, quarters)| scaled(ADAPTIVE_DP, quarters))
            .collect();
        assert_eq!(layers, [108, 162, 216, 324, 432]);
    }

    #[test]
    fn ico_sizes_are_a_subset_of_the_desktop_sizes() {
        assert!(ICO_SIZES.iter().all(|size| DESKTOP_SIZES.contains(size)));
        assert!(ICO_SIZES.iter().all(|&size| size <= 256));
    }

    #[test]
    fn adaptive_xml_references_both_layers() {
        let xml = adaptive_icon_xml();
        assert!(xml.contains("@color/ic_launcher_background"));
        assert!(xml.contains("@mipmap/ic_launcher_foreground"));
        assert!(background_color_xml().contains("#2A1240"));
    }

    #[test]
    fn rendered_icons_are_pngs() {
        let png = render_png(Variant::Rounded, 16);
        assert_eq!(&png[..8], b"\x89PNG\r\n\x1a\n");
    }
}
