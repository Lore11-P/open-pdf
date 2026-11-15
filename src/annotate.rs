use crate::engine::{self, PageSpace};
use crate::types::Annotation;
use base64::{engine::general_purpose::STANDARD as B64, Engine as _};
use pdfium_render::prelude::*;
use std::path::Path;

pub fn norm_to_page(space: &PageSpace, x: f32, y: f32) -> (f32, f32) {
    match space.rotation {
        90 => (
            space.x0 + y * space.width,
            space.y0 + x * space.height,
        ),
        180 => (
            space.x0 + (1.0 - x) * space.width,
            space.y0 + y * space.height,
        ),
        270 => (
            space.x0 + (1.0 - y) * space.width,
            space.y0 + (1.0 - x) * space.height,
        ),
        _ => (
            space.x0 + x * space.width,
            space.y0 + (1.0 - y) * space.height,
        ),
    }
}

fn rect_bbox(space: &PageSpace, x: f32, y: f32, w: f32, h: f32) -> (f32, f32, f32, f32) {
    let corners = [
        norm_to_page(space, x, y),
        norm_to_page(space, x + w, y),
        norm_to_page(space, x + w, y + h),
        norm_to_page(space, x, y + h),
    ];
    let minx = corners.iter().map(|c| c.0).fold(f32::INFINITY, f32::min);
    let maxx = corners.iter().map(|c| c.0).fold(f32::NEG_INFINITY, f32::max);
    let miny = corners.iter().map(|c| c.1).fold(f32::INFINITY, f32::min);
    let maxy = corners.iter().map(|c| c.1).fold(f32::NEG_INFINITY, f32::max);
    (minx, miny, maxx, maxy)
}

fn parse_color(hex: &str, opacity: f32) -> PdfColor {
    let h = hex.trim().trim_start_matches('#');
    let (r, g, b) = if h.len() >= 6 {
        (
            u8::from_str_radix(&h[0..2], 16).unwrap_or(255),
            u8::from_str_radix(&h[2..4], 16).unwrap_or(214),
            u8::from_str_radix(&h[4..6], 16).unwrap_or(10),
        )
    } else {
        (255, 214, 10)
    };
    PdfColor::new(r, g, b, (opacity.clamp(0.0, 1.0) * 255.0) as u8)
}

fn fill_rect<'a>(
    doc: &PdfDocument<'a>,
    page: &mut PdfPage<'a>,
    space: &PageSpace,
    x: f32,
    y: f32,
    w: f32,
    h: f32,
    color: PdfColor,
) -> Result<(), String> {
    let (left, bottom, right, top) = rect_bbox(space, x, y, w, h);
    let mut path = PdfPagePathObject::new(
        doc,
        PdfPoints::new(left),
        PdfPoints::new(bottom),
        None,
        None,
        Some(color),
    )
    .map_err(|e| e.to_string())?;
    path.rect_to(PdfPoints::new(right), PdfPoints::new(top))
        .map_err(|e| e.to_string())?;
    page.objects_mut()
        .add_path_object(path)
        .map_err(|e| e.to_string())?;
    Ok(())
}

fn stroke_rect<'a>(
    doc: &PdfDocument<'a>,
    page: &mut PdfPage<'a>,
    space: &PageSpace,
    x: f32,
    y: f32,
    w: f32,
    h: f32,
    color: PdfColor,
    width: f32,
) -> Result<(), String> {
    let (left, bottom, right, top) = rect_bbox(space, x, y, w, h);
    let mut path = PdfPagePathObject::new(
        doc,
        PdfPoints::new(left),
        PdfPoints::new(bottom),
        Some(color),
        Some(PdfPoints::new(width.max(0.4))),
        None,
    )
    .map_err(|e| e.to_string())?;
    path.rect_to(PdfPoints::new(right), PdfPoints::new(top))
        .map_err(|e| e.to_string())?;
    page.objects_mut()
        .add_path_object(path)
        .map_err(|e| e.to_string())?;
    Ok(())
}

fn stroke_line<'a>(
    doc: &PdfDocument<'a>,
    page: &mut PdfPage<'a>,
    x1: f32,
    y1: f32,
    x2: f32,
    y2: f32,
    color: PdfColor,
    width: f32,
) -> Result<(), String> {
    let mut path = PdfPagePathObject::new(
        doc,
        PdfPoints::new(x1),
        PdfPoints::new(y1),
        Some(color),
        Some(PdfPoints::new(width.max(0.4))),
        None,
    )
    .map_err(|e| e.to_string())?;
    path.set_line_cap(PdfPageObjectLineCap::Round)
        .map_err(|e| e.to_string())?;
    path.set_line_join(PdfPageObjectLineJoin::Round)
        .map_err(|e| e.to_string())?;
    path.line_to(PdfPoints::new(x2), PdfPoints::new(y2))
        .map_err(|e| e.to_string())?;
    page.objects_mut()
        .add_path_object(path)
        .map_err(|e| e.to_string())?;
    Ok(())
}

fn add_text<'a>(
    doc: &PdfDocument<'a>,
    page: &mut PdfPage<'a>,
    token: PdfFontToken,
    text: &str,
    x: f32,
    y: f32,
    size: f32,
    color: PdfColor,
    rotation: i32,
) -> Result<(), String> {
    if text.trim().is_empty() {
        return Ok(());
    }
    let mut object = PdfPageTextObject::new(
        doc,
        text.replace(['\r', '\n'], " "),
        token,
        PdfPoints::new(size.max(4.0)),
    )
    .map_err(|e| e.to_string())?;
    object.set_fill_color(color).map_err(|e| e.to_string())?;
    if rotation != 0 {
        object
            .rotate_counter_clockwise_degrees(rotation as f32)
            .map_err(|e| e.to_string())?;
    }
    object
        .translate(PdfPoints::new(x), PdfPoints::new(y))
        .map_err(|e| e.to_string())?;
    page.objects_mut()
        .add_text_object(object)
        .map_err(|e| e.to_string())?;
    Ok(())
}

fn wrap(text: &str, max_chars: usize) -> Vec<String> {
    let mut lines = vec![];
    for raw in text.split('\n') {
        let mut line = String::new();
        for word in raw.split_whitespace() {
            if line.is_empty() {
                line.push_str(word);
            } else if line.chars().count() + 1 + word.chars().count() <= max_chars {
                line.push(' ');
                line.push_str(word);
            } else {
                lines.push(line);
                line = word.to_string();
            }
        }
        lines.push(line);
    }
    if lines.is_empty() {
        lines.push(String::new());
    }
    lines
}

fn data_url_image(data_url: &str) -> Option<image::DynamicImage> {
    let data = data_url.split_once(',').map(|(_, d)| d).unwrap_or(data_url);
    let bytes = B64.decode(data.trim()).ok()?;
    image::load_from_memory(&bytes).ok()
}

fn add_image<'a>(
    doc: &PdfDocument<'a>,
    page: &mut PdfPage<'a>,
    space: &PageSpace,
    ann: &Annotation,
    img: &image::DynamicImage,
) -> Result<(), String> {
    let nw = ann.w.max(0.02);
    let nh = ann.h.max(0.01);
    let local_w = nw * space.display_width;
    let local_h = nh * space.display_height;
    let (left, bottom, right, top) = rect_bbox(space, ann.x, ann.y, nw, nh);
    let mut object = PdfPageImageObject::new_with_size(
        doc,
        img,
        PdfPoints::new(local_w),
        PdfPoints::new(local_h),
    )
    .map_err(|e| e.to_string())?;
    let (tx, ty) = match space.rotation {
        90 => (right, bottom),
        180 => (right, top),
        270 => (left, top),
        _ => (left, bottom),
    };
    if space.rotation != 0 {
        object
            .rotate_counter_clockwise_degrees(space.rotation as f32)
            .map_err(|e| e.to_string())?;
    }
    object
        .translate(PdfPoints::new(tx), PdfPoints::new(ty))
        .map_err(|e| e.to_string())?;
    page.objects_mut()
        .add_image_object(object)
        .map_err(|e| e.to_string())?;
    Ok(())
}

pub fn burn_annotations(
    src: &Path,
    dest: &Path,
    password: Option<&str>,
    annotations: &[Annotation],
) -> Result<(), String> {
    let mut doc = engine::open_doc(src, password)?;
    let regular = doc.fonts_mut().new_built_in(PdfFontBuiltin::Helvetica);
    let bold = doc.fonts_mut().new_built_in(PdfFontBuiltin::HelveticaBold);
    let count = doc.pages().len();
    for page_index in 0..count {
        let page_annotations: Vec<&Annotation> = annotations
            .iter()
            .filter(|a| a.page == page_index as usize)
            .collect();
        if page_annotations.is_empty() {
            continue;
        }
        let mut page = doc.pages_mut().get(page_index).map_err(|e| e.to_string())?;
        let space = engine::page_space(&page)?;
        for ann in page_annotations {
            let color = parse_color(&ann.color, ann.opacity);
            let solid = parse_color(&ann.color, 1.0);
            let black = PdfColor::new(8, 10, 16, 255);
            match ann.kind.as_str() {
                "highlight" => {
                    fill_rect(&doc, &mut page, &space, ann.x, ann.y, ann.w, ann.h, color)?;
                }
                "redact" => {
                    fill_rect(&doc, &mut page, &space, ann.x, ann.y, ann.w, ann.h, black)?;
                }
                "underline" => {
                    let y = ann.y + ann.h;
                    let (x1, y1) = norm_to_page(&space, ann.x, y);
                    let (x2, y2) = norm_to_page(&space, ann.x + ann.w, y);
                    stroke_line(
                        &doc,
                        &mut page,
                        x1,
                        y1,
                        x2,
                        y2,
                        solid,
                        ann.stroke * space.display_width,
                    )?;
                }
                "ink" => {
                    if ann.points.len() > 1 {
                        let (px, py) = norm_to_page(&space, ann.points[0][0], ann.points[0][1]);
                        let mut path = PdfPagePathObject::new(
                            &doc,
                            PdfPoints::new(px),
                            PdfPoints::new(py),
                            Some(solid),
                            Some(PdfPoints::new((ann.stroke * space.display_width).max(0.5))),
                            None,
                        )
                        .map_err(|e| e.to_string())?;
                        path.set_line_cap(PdfPageObjectLineCap::Round)
                            .map_err(|e| e.to_string())?;
                        path.set_line_join(PdfPageObjectLineJoin::Round)
                            .map_err(|e| e.to_string())?;
                        for point in ann.points.iter().skip(1) {
                            let (x, y) = norm_to_page(&space, point[0], point[1]);
                            path.line_to(PdfPoints::new(x), PdfPoints::new(y))
                                .map_err(|e| e.to_string())?;
                        }

                        page.objects_mut()
                            .add_path_object(path)
                            .map_err(|e| e.to_string())?;
                    }
                }
                "note" => {
                    fill_rect(
                        &doc,
                        &mut page,
                        &space,
                        ann.x,
                        ann.y,
                        ann.w.max(0.05),
                        ann.h.max(0.035),
                        PdfColor::new(222, 211, 180, 238),
                    )?;
                    stroke_rect(
                        &doc,
                        &mut page,
                        &space,
                        ann.x,
                        ann.y,
                        ann.w.max(0.05),
                        ann.h.max(0.035),
                        PdfColor::new(110, 98, 76, 255),
                        0.8,
                    )?;
                    let fs_norm = if ann.font_size > 0.001 {
                        ann.font_size
                    } else {
                        (ann.h * 0.11).clamp(0.006, 0.03)
                    };
                    let fs = (fs_norm * space.display_height).clamp(6.0, 18.0);
                    let max_chars = ((ann.w.max(0.05) * space.display_width) / (fs * 0.52)).max(6.0) as usize;
                    for (i, line) in wrap(&ann.text, max_chars).iter().take(8).enumerate() {
                        let by = ann.y + fs_norm * (0.9 + i as f32 * 1.25);
                        let (x, y) = norm_to_page(&space, ann.x + fs_norm * 0.3, by);
                        add_text(
                            &doc,
                            &mut page,
                            regular,
                            line,
                            x,
                            y,
                            fs,
                            PdfColor::new(50, 45, 35, 255),
                            space.rotation,
                        )?;
                    }
                }
                "text" => {
                    let fs_norm = if ann.font_size > 0.001 { ann.font_size } else { 0.018 };
                    let fs_pt = (fs_norm * space.display_height).clamp(5.0, 120.0);
                    let max_chars =
                        ((ann.w.max(0.03) * space.display_width) / (fs_pt * 0.52)).max(4.0) as usize;
                    for (i, line) in wrap(&ann.text, max_chars).iter().enumerate() {
                        let by = ann.y + fs_norm * (0.95 + i as f32 * 1.3);
                        let (x, y) = norm_to_page(&space, ann.x, by);
                        add_text(
                            &doc,
                            &mut page,
                            regular,
                            line,
                            x,
                            y,
                            fs_pt,
                            solid,
                            space.rotation,
                        )?;
                    }
                }
                "link" => {
                    let fs_pt = (ann.font_size * space.display_height).clamp(5.0, 80.0);
                    let (x, y) = norm_to_page(&space, ann.x, ann.y + ann.font_size);
                    add_text(
                        &doc,
                        &mut page,
                        regular,
                        &ann.text,
                        x,
                        y,
                        fs_pt,
                        PdfColor::new(56, 130, 246, 255),
                        space.rotation,
                    )?;
                }
                "stamp" => {
                    stroke_rect(
                        &doc,
                        &mut page,
                        &space,
                        ann.x,
                        ann.y,
                        ann.w,
                        ann.h,
                        solid,
                        (ann.stroke * space.display_width).max(1.0),
                    )?;
                    let label = if ann.text.trim().is_empty() {
                        ann.label.clone()
                    } else {
                        ann.text.clone()
                    };
                    let fs_pt = (ann.h * space.display_height * 0.42).clamp(8.0, 42.0);
                    let (x, y) = norm_to_page(
                        &space,
                        ann.x + ann.w * 0.5
                            - (label.chars().count() as f32 * fs_pt * 0.27 / space.display_width),
                        ann.y + ann.h * 0.62,
                    );
                    add_text(
                        &doc,
                        &mut page,
                        bold,
                        &label.to_uppercase(),
                        x,
                        y,
                        fs_pt,
                        solid,
                        space.rotation,
                    )?;
                }
                "signature" => {
                    if let Some(img) = data_url_image(&ann.data_url) {
                        add_image(&doc, &mut page, &space, ann, &img)?;
                    }
                }
                _ => {}
            }
        }
        page.regenerate_content().map_err(|e| e.to_string())?;
    }
    doc.save_to_file(dest).map_err(|e| e.to_string())?;
    Ok(())
}



