use crate::engine;
use base64::{engine::general_purpose::STANDARD as B64, Engine as _};
use image::DynamicImage;
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::Command;

const WIC_SCRIPT: &str = r#"
$ErrorActionPreference='Stop'
Add-Type -AssemblyName PresentationCore
Add-Type -AssemblyName WindowsBase
$in=$args[0]; $out=$args[1]
$stream=[System.IO.File]::OpenRead($in)
try {
  $decoder=[System.Windows.Media.Imaging.BitmapDecoder]::Create($stream,[System.Windows.Media.Imaging.BitmapCreateOptions]::PreservePixelFormat,[System.Windows.Media.Imaging.BitmapCacheOption]::OnLoad)
  $frame=$decoder.Frames[0]
  $max=[Math]::Max($frame.PixelWidth,$frame.PixelHeight)
  if($max -gt 2400){
    $scale=2400.0/$max
    $frame=New-Object System.Windows.Media.Imaging.TransformedBitmap($frame,(New-Object System.Windows.Media.ScaleTransform($scale,$scale)))
  }
  $encoder=New-Object System.Windows.Media.Imaging.PngBitmapEncoder
  $encoder.Frames.Add([System.Windows.Media.Imaging.BitmapFrame]::Create($frame))
  $file=[System.IO.File]::Create($out)
  $encoder.Save($file); $file.Close()
} finally { $stream.Close() }
"#;

const GDI_SCRIPT: &str = r#"
$ErrorActionPreference='Stop'
Add-Type -AssemblyName System.Drawing
$in=$args[0]; $out=$args[1]
$img=[System.Drawing.Image]::FromFile($in)
try {
  $w=$img.Width; $h=$img.Height
  $max=[Math]::Max($w,$h)
  if($max -gt 2400){ $scale=2400.0/$max; $w=[int]($w*$scale); $h=[int]($h*$scale) }
  $bmp=New-Object System.Drawing.Bitmap($w,$h)
  $g=[System.Drawing.Graphics]::FromImage($bmp)
  $g.Clear([System.Drawing.Color]::White)
  $g.DrawImage($img,0,0,$w,$h)
  $g.Dispose()
  $bmp.Save($out,[System.Drawing.Imaging.ImageFormat]::Png)
  $bmp.Dispose()
} finally { $img.Dispose() }
"#;

fn extension(path: &Path) -> String {
    path.extension()
        .map(|value| value.to_string_lossy().to_lowercase())
        .unwrap_or_default()
}

fn temp_file(extension: &str) -> PathBuf {
    let dir = std::env::temp_dir().join("openpdf-convert");
    let _ = fs::create_dir_all(&dir);
    dir.join(format!(
        "{}.{}",
        uuid::Uuid::new_v4(),
        extension.trim_start_matches('.')
    ))
}

fn run_powershell(script: &str, inputs: &[&Path], output_path: &Path) -> Result<(), String> {
    let dir = std::env::temp_dir().join("openpdf-scripts");
    let _ = fs::create_dir_all(&dir);
    let script_path = dir.join(format!("{}.ps1", uuid::Uuid::new_v4()));
    fs::write(&script_path, script).map_err(|e| e.to_string())?;
    let mut command = Command::new("powershell.exe");
    command.args([
        "-NoProfile",
        "-ExecutionPolicy",
        "Bypass",
        "-File",
        &script_path.to_string_lossy(),
    ]);
    for input in inputs {
        command.arg(input.as_os_str());
    }
    command.arg(output_path.as_os_str());
    let result = command.output();
    let _ = fs::remove_file(&script_path);
    let output = result.map_err(|e| format!("Conversione non avviata: {e}"))?;
    if output.status.success() && output_path.exists() {
        Ok(())
    } else {
        Err(String::from_utf8_lossy(&output.stderr).trim().to_string())
    }
}

fn decode_file(path: &Path) -> Result<DynamicImage, String> {
    image::open(path).map_err(|e| e.to_string())
}

fn convert_wic(path: &Path) -> Result<DynamicImage, String> {
    let target = temp_file("png");
    run_powershell(WIC_SCRIPT, &[path], &target)?;
    let result = decode_file(&target);
    let _ = fs::remove_file(&target);
    result
}

fn convert_gdi(path: &Path) -> Result<DynamicImage, String> {
    let target = temp_file("png");
    run_powershell(GDI_SCRIPT, &[path], &target)?;
    let result = decode_file(&target);
    let _ = fs::remove_file(&target);
    result
}

fn convert_ffmpeg(path: &Path) -> Result<DynamicImage, String> {
    let ffmpeg = engine::find_executable(&["ffmpeg.exe", "ffmpeg"])
        .ok_or_else(|| "FFmpeg non trovato".to_string())?;
    let target = temp_file("png");
    let output = Command::new(ffmpeg)
        .args([
            "-y",
            "-v",
            "error",
            "-i",
            &path.to_string_lossy(),
            "-frames:v",
            "1",
            "-vf",
            "scale='min(2400,iw)':-2",
            &target.to_string_lossy(),
        ])
        .output()
        .map_err(|e| format!("FFmpeg non avviato: {e}"))?;
    if !output.status.success() || !target.exists() {
        let _ = fs::remove_file(&target);
        return Err(String::from_utf8_lossy(&output.stderr).trim().to_string());
    }
    let result = decode_file(&target);
    let _ = fs::remove_file(&target);
    result
}

fn convert_magick(path: &Path) -> Result<DynamicImage, String> {
    let magick = engine::find_executable(&["magick.exe", "magick"])
        .ok_or_else(|| "ImageMagick non trovato".to_string())?;
    let target = temp_file("png");
    let input = format!("{}[0]", path.display());
    let output = Command::new(magick)
        .args([
            &input,
            "-auto-orient",
            "-resize",
            "2400x2400>",
            "-background",
            "white",
            "-alpha",
            "remove",
            "-alpha",
            "off",
            &target.to_string_lossy(),
        ])
        .output()
        .map_err(|e| format!("ImageMagick non avviato: {e}"))?;
    if !output.status.success() || !target.exists() {
        let _ = fs::remove_file(&target);
        return Err(String::from_utf8_lossy(&output.stderr).trim().to_string());
    }
    let result = decode_file(&target);
    let _ = fs::remove_file(&target);
    result
}

fn convert_ghostscript(path: &Path) -> Result<DynamicImage, String> {
    let gs = engine::find_executable(&["gswin64c.exe", "gswin64c", "gswin32c.exe"])
        .ok_or_else(|| "Ghostscript non trovato".to_string())?;
    let target = temp_file("png");
    let output = Command::new(gs)
        .args([
            "-dSAFER",
            "-dBATCH",
            "-dNOPAUSE",
            "-sDEVICE=pngalpha",
            "-r150",
            "-dEPSCrop",
            &format!("-sOutputFile={}", target.display()),
            &path.to_string_lossy(),
        ])
        .output()
        .map_err(|e| format!("Ghostscript non avviato: {e}"))?;
    if !output.status.success() || !target.exists() {
        let _ = fs::remove_file(&target);
        return Err(String::from_utf8_lossy(&output.stderr).trim().to_string());
    }
    let result = decode_file(&target);
    let _ = fs::remove_file(&target);
    result
}

fn render_pdf(path: &Path) -> Result<DynamicImage, String> {
    let doc = engine::open_doc(path, None)?;
    if doc.pages().len() == 0 {
        return Err("Il PDF non contiene pagine.".into());
    }
    engine::render_page_image(&doc, 0, 2400)
}

fn render_svg(path: &Path) -> Result<DynamicImage, String> {
    let edge = engine::edge_path().ok_or("Microsoft Edge non trovato per convertire l'SVG.")?;
    let source = fs::read(path).map_err(|e| format!("SVG non leggibile: {e}"))?;
    let encoded = B64.encode(source);
    let html = format!(
        "<!doctype html><html><head><meta charset=\"utf-8\"><style>html,body{{margin:0;width:1600px;height:1200px;background:#fff}}body{{display:flex;align-items:center;justify-content:center}}img{{max-width:1520px;max-height:1120px;width:auto;height:auto}}</style></head><body><img src=\"data:image/svg+xml;base64,{encoded}\"></body></html>"
    );
    let html_path = temp_file("html");
    let target = temp_file("png");
    fs::write(&html_path, html).map_err(|e| e.to_string())?;
    let url = format!("file:///{}", html_path.to_string_lossy().replace('\\', "/").replace(' ', "%20"));
    let output = Command::new(edge)
        .args([
            "--headless=new",
            "--disable-gpu",
            "--hide-scrollbars",
            "--window-size=1600,1200",
            "--virtual-time-budget=3000",
            &format!("--screenshot={}", target.display()),
            &url,
        ])
        .output()
        .map_err(|e| format!("Edge non avviato: {e}"))?;
    if !output.status.success() || !target.exists() {
        let _ = fs::remove_file(&html_path);
        let _ = fs::remove_file(&target);
        return Err("Conversione SVG non riuscita.".into());
    }
    let result = decode_file(&target);
    let _ = fs::remove_file(&html_path);
    let _ = fs::remove_file(&target);
    result
}

fn load_zip_image(path: &Path) -> Result<DynamicImage, String> {
    let file = fs::File::open(path).map_err(|e| e.to_string())?;
    let mut archive = zip::ZipArchive::new(file).map_err(|e| format!("Archivio non valido: {e}"))?;
    let preferred = ["mergedimage.png", "preview.png", "mergedimage.jpg"];
    for name in preferred {
        if let Ok(mut entry) = archive.by_name(name) {
            let mut bytes = Vec::new();
            entry.read_to_end(&mut bytes).map_err(|e| e.to_string())?;
            return image::load_from_memory(&bytes).map_err(|e| e.to_string());
        }
    }
    let mut candidates = Vec::new();
    for index in 0..archive.len() {
        if let Ok(entry) = archive.by_index(index) {
            let name = entry.name().to_lowercase();
            let ext = Path::new(&name)
                .extension()
                .map(|v| v.to_string_lossy().to_lowercase())
                .unwrap_or_default();
            if ["png", "jpg", "jpeg", "heic", "heif", "webp", "tif", "tiff"].contains(&ext.as_str()) {
                candidates.push(entry.name().to_string());
            }
        }
    }
    for name in candidates {
        let mut entry = archive.by_name(&name).map_err(|e| e.to_string())?;
        let mut bytes = Vec::new();
        entry.read_to_end(&mut bytes).map_err(|e| e.to_string())?;
        if let Ok(image) = image::load_from_memory(&bytes) {
            return Ok(image);
        }
        let ext = Path::new(&name)
            .extension()
            .map(|v| v.to_string_lossy().to_lowercase())
            .unwrap_or_else(|| "bin".into());
        let temp = temp_file(&ext);
        fs::write(&temp, bytes).map_err(|e| e.to_string())?;
        let loaded = load_image_any(&temp);
        let _ = fs::remove_file(&temp);
        if let Ok(image) = loaded {
            return Ok(image);
        }
    }
    Err("Nessuna immagine incorporata trovata nell'archivio.".into())
}

fn load_icns(path: &Path) -> Result<DynamicImage, String> {
    let bytes = fs::read(path).map_err(|e| e.to_string())?;
    if bytes.len() < 8 || &bytes[0..4] != b"icns" {
        return Err("File ICNS non valido.".into());
    }
    let mut offset = 8usize;
    let mut best: Option<DynamicImage> = None;
    while offset + 8 <= bytes.len() {
        let size = u32::from_be_bytes([bytes[offset + 4], bytes[offset + 5], bytes[offset + 6], bytes[offset + 7]]) as usize;
        if size < 8 || offset + size > bytes.len() {
            break;
        }
        let payload = &bytes[offset + 8..offset + size];
        if payload.starts_with(&[0x89, b'P', b'N', b'G']) {
            if let Ok(image) = image::load_from_memory(payload) {
                let better = best
                    .as_ref()
                    .map(|current| image.width() as u64 * image.height() as u64 > current.width() as u64 * current.height() as u64)
                    .unwrap_or(true);
                if better {
                    best = Some(image);
                }
            }
        }
        offset += size;
    }
    best.ok_or("Nessuna icona PNG incorporata nel file ICNS.".into())
}

pub fn load_image_any(path: &Path) -> Result<DynamicImage, String> {
    let ext = extension(path);
    match ext.as_str() {
        "pdf" => return render_pdf(path),
        "svg" => return render_svg(path),
        "kra" => return load_zip_image(path),
        "livp" => return load_zip_image(path),
        "icns" => return load_icns(path),
        "wmf" | "emf" => {
            if let Ok(image) = convert_gdi(path) {
                return Ok(image);
            }
        }
        _ => {}
    }

    if let Ok(image) = image::open(path) {
        return Ok(image);
    }

    if ext == "wmf" || ext == "emf" {
        if let Ok(image) = convert_gdi(path) {
            return Ok(image);
        }
    }
    if let Ok(image) = convert_wic(path) {
        return Ok(image);
    }
    if let Ok(image) = convert_ffmpeg(path) {
        return Ok(image);
    }
    if let Ok(image) = convert_magick(path) {
        return Ok(image);
    }
    if ext == "ai" || ext == "eps" {
        if let Ok(image) = convert_ghostscript(path) {
            return Ok(image);
        }
    }

    let label = if ext.is_empty() {
        "il file senza estensione".to_string()
    } else {
        format!("il file .{ext}")
    };
    let message = match ext.as_str() {
        "heic" | "heif" | "avif" | "dng" | "cr2" | "cr3" | "nef" | "arw" | "raf" | "orf"
        | "rw2" | "pef" | "srw" | "3fr" | "iiq" | "raw" | "proraw" => {
            "Serve il codec Windows per questo formato (HEIF/RAW Image Extension) oppure converti la foto in JPEG/PNG."
        }
        "ai" | "eps" => "Per importare AI/EPS serve Ghostscript installato.",
        "cdr" | "indd" | "psb" | "xcf" | "dcm" | "dicom" => {
            "Questo formato professionale richiede ImageMagick o l'app originale per essere convertito."
        }
        _ => "Formato non supportato su questo computer. Usa JPEG, PNG, WebP, HEIC, TIFF o converti la foto in PNG.",
    };
    Err(format!("Impossibile leggere {label}. {message}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn loads_svg_with_edge() {
        if crate::engine::edge_path().is_none() {
            return;
        }
        let dir = std::env::temp_dir().join("openpdf-loader-svg");
        let _ = fs::create_dir_all(&dir);
        let svg = dir.join("firma.svg");
        fs::write(
            &svg,
            "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"800\" height=\"300\"><rect width=\"100%\" height=\"100%\" fill=\"white\"/><path d=\"M80 200 C 180 40, 300 260, 420 120 S 600 60, 720 180\" fill=\"none\" stroke=\"#111\" stroke-width=\"8\" stroke-linecap=\"round\"/></svg>",
        )
        .unwrap();
        let image = load_image_any(&svg).unwrap();
        assert!(image.width() > 200);
        assert!(image.height() > 100);
    }

    #[test]
    fn loads_kra_preview() {
        let dir = std::env::temp_dir().join("openpdf-loader-kra");
        let _ = fs::create_dir_all(&dir);
        let png = dir.join("preview.png");
        let image = image::RgbaImage::from_pixel(60, 30, image::Rgba([15, 20, 30, 255]));
        image.save(&png).unwrap();
        let kra = dir.join("test.kra");
        let file = fs::File::create(&kra).unwrap();
        let mut zip = zip::ZipWriter::new(file);
        zip.start_file("mergedimage.png", zip::write::SimpleFileOptions::default())
            .unwrap();
        use std::io::Write;
        zip.write_all(&fs::read(&png).unwrap()).unwrap();
        zip.finish().unwrap();
        let loaded = load_image_any(&kra).unwrap();
        assert_eq!((loaded.width(), loaded.height()), (60, 30));
    }

    #[test]
    fn loads_modern_formats_with_ffmpeg() {
        let Some(ffmpeg) = crate::engine::find_executable(&["ffmpeg.exe", "ffmpeg"]) else {
            return;
        };
        let dir = std::env::temp_dir().join("openpdf-loader-modern");
        let _ = fs::create_dir_all(&dir);
        let png = dir.join("source.png");
        let image = image::RgbaImage::from_pixel(120, 60, image::Rgba([220, 210, 190, 255]));
        image.save(&png).unwrap();
        for (extension, codec, format) in [
            ("avif", "libaom-av1", "avif"),
            ("jp2", "jpeg2000", "image2"),
            ("jxl", "libjxl", "image2"),
        ] {
            let target = dir.join(format!("photo.{extension}"));
            let status = Command::new(&ffmpeg)
                .args([
                    "-y",
                    "-v",
                    "error",
                    "-i",
                    &png.to_string_lossy(),
                    "-frames:v",
                    "1",
                    "-c:v",
                    codec,
                    "-f",
                    format,
                    &target.to_string_lossy(),
                ])
                .status();
            if !matches!(status, Ok(status) if status.success()) {
                continue;
            }
            let loaded = load_image_any(&target).unwrap();
            assert!(loaded.width() >= 20);
            assert!(loaded.height() >= 10);
        }
    }

    #[test]
    fn loads_png_and_pdf() {
        let dir = std::env::temp_dir().join("openpdf-loader-test");
        let _ = fs::create_dir_all(&dir);
        let png = dir.join("test.png");
        let image = image::RgbaImage::from_pixel(40, 20, image::Rgba([20, 30, 40, 255]));
        image.save(&png).unwrap();
        let loaded = load_image_any(&png).unwrap();
        assert_eq!((loaded.width(), loaded.height()), (40, 20));

        let pdf = dir.join("test.pdf");
        crate::convert::text_to_pdf(&pdf, "Test", "Documento di prova").unwrap();
        let rendered = load_image_any(&pdf).unwrap();
        assert!(rendered.width() > 100);
    }
}


