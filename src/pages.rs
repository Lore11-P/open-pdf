use crate::engine;
use pdfium_render::prelude::*;
use std::path::Path;

fn err(e: impl std::fmt::Display) -> String {
    format!("Operazione sulle pagine non riuscita: {e}")
}

pub fn copy_pages(src: &Path, dest: &Path, order: &[usize], password: Option<&str>) -> Result<(), String> {
    let pdfium = engine::bind()?;
    let source = engine::open_doc(src, password)?;
    let mut out = pdfium.create_new_pdf().map_err(err)?;
    for &index in order {
        let at = out.pages().len();
        out.pages_mut()
            .copy_page_from_document(&source, index as i32, at)
            .map_err(err)?;
    }
    out.save_to_file(dest).map_err(err)?;
    Ok(())
}

pub fn rotate_page(
    src: &Path,
    dest: &Path,
    page_index: usize,
    delta: i32,
    password: Option<&str>,
) -> Result<(), String> {
    let mut doc = engine::open_doc(src, password)?;
    {
        let mut page = doc.pages_mut().get(page_index as i32).map_err(err)?;
        let current = match page.rotation().map_err(err)? {
            PdfPageRenderRotation::None => 0,
            PdfPageRenderRotation::Degrees90 => 90,
            PdfPageRenderRotation::Degrees180 => 180,
            PdfPageRenderRotation::Degrees270 => 270,
        };
        let next = ((current + delta) % 360 + 360) % 360;
        page.set_rotation(match next {
            90 => PdfPageRenderRotation::Degrees90,
            180 => PdfPageRenderRotation::Degrees180,
            270 => PdfPageRenderRotation::Degrees270,
            _ => PdfPageRenderRotation::None,
        });
    }
    doc.save_to_file(dest).map_err(err)?;
    Ok(())
}

pub fn merge_pdfs(dest: &Path, sources: &[String]) -> Result<(), String> {
    let pdfium = engine::bind()?;
    let mut out = pdfium.create_new_pdf().map_err(err)?;
    for source_path in sources {
        let source = engine::open_doc(Path::new(source_path), None)?;
        out.pages_mut().append(&source).map_err(err)?;
    }
    out.save_to_file(dest).map_err(err)?;
    Ok(())
}

pub fn split_pdf(
    src: &Path,
    dest: &Path,
    from: usize,
    to: usize,
    password: Option<&str>,
) -> Result<(), String> {
    let order: Vec<usize> = (from..=to).collect();
    copy_pages(src, dest, &order, password)
}

pub fn images_to_pdf(dest: &Path, images: &[String]) -> Result<(), String> {
    let pdfium = engine::bind()?;
    let mut out = pdfium.create_new_pdf().map_err(err)?;
    for path in images {
        let img = image::open(path).map_err(|e| format!("Immagine non valida: {e}"))?;
        let w = (img.width() as f32 * 0.75).clamp(120.0, 2400.0);
        let h = (img.height() as f32 * 0.75).clamp(120.0, 3400.0);
        let page_size = PdfPagePaperSize::new_custom(PdfPoints::new(w), PdfPoints::new(h));
        {
            let mut page = out.pages_mut().create_page_at_end(page_size).map_err(err)?;
            let mut object =
                PdfPageImageObject::new_with_size(&out, &img, PdfPoints::new(w), PdfPoints::new(h))
                    .map_err(err)?;
            object
                .translate(PdfPoints::new(0.0), PdfPoints::new(0.0))
                .map_err(err)?;
            page.objects_mut().add_image_object(object).map_err(err)?;
            page.regenerate_content().map_err(err)?;
        }
    }
    out.save_to_file(dest).map_err(err)?;
    Ok(())
}

pub fn compress_lossless(src: &Path, dest: &Path, password: Option<&str>) -> Result<(u64, u64), String> {
    let before = std::fs::metadata(src).map(|m| m.len()).unwrap_or(0);
    let mut doc = match password {
        Some(p) => lopdf::Document::load_with_password(src, p),
        None => lopdf::Document::load(src),
    }
    .map_err(|e| format!("Compressione non riuscita: {e}"))?;
    doc.compress();
    doc.prune_objects();
    doc.delete_zero_length_streams();
    if let Ok(info) = doc.trailer.get(b"Info").and_then(|o| o.as_reference()) {
        let _ = doc.delete_object(info);
    }
    if let Ok(root_id) = doc.trailer.get(b"Root").and_then(|o| o.as_reference()) {
        if let Ok(root) = doc.get_dictionary_mut(root_id) {
            root.remove(b"Metadata");
        }
    }
    let options = lopdf::SaveOptions::builder()
        .use_object_streams(true)
        .use_xref_streams(true)
        .compression_level(9)
        .build();
    let file = std::fs::File::create(dest).map_err(|e| e.to_string())?;
    let mut writer = std::io::BufWriter::new(file);
    doc.save_with_options(&mut writer, options)
        .map_err(|e| format!("Compressione non riuscita: {e}"))?;
    drop(writer);
    let after = std::fs::metadata(dest).map(|m| m.len()).unwrap_or(0);
    Ok((before, after))
}

fn fill_black(img: &mut image::DynamicImage, nx: f32, ny: f32, nw: f32, nh: f32) {
    let w = img.width() as f32;
    let h = img.height() as f32;
    let x0 = (nx * w).floor().max(0.0) as u32;
    let y0 = (ny * h).floor().max(0.0) as u32;
    let x1 = ((nx + nw) * w).ceil().min(w) as u32;
    let y1 = ((ny + nh) * h).ceil().min(h) as u32;
    let rgb = img.to_rgb8();
    let mut rgb = rgb;
    for y in y0..y1 {
        for x in x0..x1 {
            rgb.put_pixel(x, y, image::Rgb([0, 0, 0]));
        }
    }
    *img = image::DynamicImage::ImageRgb8(rgb);
}

fn jpeg_bytes(img: &image::DynamicImage, quality: u8) -> Result<Vec<u8>, String> {
    let mut out = Vec::new();
    let rgb = img.to_rgb8();
    let mut encoder = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, quality);
    encoder
        .encode(rgb.as_raw(), rgb.width(), rgb.height(), image::ExtendedColorType::Rgb8)
        .map_err(|e| e.to_string())?;
    Ok(out)
}

fn rebuild_with_images(
    source: &Path,
    dest: &Path,
    password: Option<&str>,
    dpi: f32,
    quality: u8,
    redactions: &[([f32; 4], usize)],
) -> Result<(), String> {
    let pdfium = engine::bind()?;
    let doc = engine::open_doc(source, password)?;
    let count = doc.pages().len();
    let mut out = pdfium.create_new_pdf().map_err(err)?;
    for i in 0..count {
        let page = doc.pages().get(i).map_err(err)?;
        let space = engine::page_space(&page)?;
        let width_px = ((space.display_width / 72.0) * dpi).clamp(200.0, 4200.0) as u32;
        let mut img = engine::render_page_image(&doc, i as usize, width_px)?;
        for (rect, page_idx) in redactions {
            if *page_idx == i as usize {
                fill_black(&mut img, rect[0], rect[1], rect[2], rect[3]);
            }
        }
        let jpeg = jpeg_bytes(&img, quality)?;
        let decoded = image::load_from_memory(&jpeg).map_err(|e| e.to_string())?;
        let page_size = PdfPagePaperSize::new_custom(
            PdfPoints::new(space.display_width),
            PdfPoints::new(space.display_height),
        );
        {
            let mut new_page = out.pages_mut().create_page_at_end(page_size).map_err(err)?;
            let mut object = PdfPageImageObject::new_with_size(
                &out,
                &decoded,
                PdfPoints::new(space.display_width),
                PdfPoints::new(space.display_height),
            )
            .map_err(err)?;
            object
                .translate(PdfPoints::new(0.0), PdfPoints::new(0.0))
                .map_err(err)?;
            new_page.objects_mut().add_image_object(object).map_err(err)?;
            new_page.regenerate_content().map_err(err)?;
        }
    }
    out.save_to_file(dest).map_err(err)?;
    Ok(())
}

pub fn compress_raster(
    src: &Path,
    dest: &Path,
    password: Option<&str>,
    dpi: f32,
    quality: u8,
) -> Result<(u64, u64), String> {
    let before = std::fs::metadata(src).map(|m| m.len()).unwrap_or(0);
    rebuild_with_images(src, dest, password, dpi, quality, &[])?;
    let after = std::fs::metadata(dest).map(|m| m.len()).unwrap_or(0);
    Ok((before, after))
}

pub fn redact_pdf(
    src: &Path,
    dest: &Path,
    password: Option<&str>,
    rects: &[([f32; 4], usize)],
    dpi: f32,
) -> Result<(), String> {
    rebuild_with_images(src, dest, password, dpi, 90, rects)?;
    let before = std::fs::metadata(src).map(|m| m.len()).unwrap_or(0);
    let after = std::fs::metadata(dest).map(|m| m.len()).unwrap_or(0);
    let _ = (before, after);
    if let Ok(mut doc) = lopdf::Document::load(dest) {
        if let Ok(root_id) = doc.trailer.get(b"Root").and_then(|o| o.as_reference()) {
            if let Ok(root) = doc.get_dictionary_mut(root_id) {
                root.remove(b"Metadata");
            }
        }
        if let Ok(info) = doc.trailer.get(b"Info").and_then(|o| o.as_reference()) {
            let _ = doc.delete_object(info);
        }
        let _ = doc.save(dest);
    }
    Ok(())
}

