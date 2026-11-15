use base64::{engine::general_purpose::STANDARD as B64, Engine as _};
use image::{imageops::FilterType, GrayImage, Rgba, RgbaImage};
use std::collections::{HashMap, VecDeque};
use std::io::Cursor;
use std::path::Path;

const MAX_INPUT_SIDE: u32 = 1800;
const MAX_OUTPUT_SIDE: u32 = 1400;
const INK: [u8; 3] = [16, 18, 22];

struct Component {
    area: u32,
    min_x: u32,
    min_y: u32,
    max_x: u32,
    max_y: u32,
    keep: bool,
}

fn no_signature_error() -> String {
    "Nessuna firma riconosciuta. Fotografa la firma da vicino, con inchiostro scuro su carta chiara e luce uniforme.".into()
}

fn otsu(hist: &[u64; 256], total: u64) -> u8 {
    let sum: u64 = hist
        .iter()
        .enumerate()
        .map(|(value, count)| value as u64 * *count)
        .sum();
    let mut weight_background = 0u64;
    let mut sum_background = 0u64;
    let mut best_variance = -1.0f64;
    let mut best = 128u8;

    for threshold in 0..256usize {
        weight_background += hist[threshold];
        if weight_background == 0 {
            continue;
        }
        let weight_foreground = total.saturating_sub(weight_background);
        if weight_foreground == 0 {
            break;
        }
        sum_background += threshold as u64 * hist[threshold];
        let mean_background = sum_background as f64 / weight_background as f64;
        let mean_foreground = (sum - sum_background) as f64 / weight_foreground as f64;
        let variance =
            weight_background as f64 * weight_foreground as f64 * (mean_background - mean_foreground).powi(2);
        if variance > best_variance {
            best_variance = variance;
            best = threshold as u8;
        }
    }
    best
}

fn sliding_extreme(input: &[u8], width: usize, height: usize, radius: usize, maximum: bool) -> Vec<u8> {
    let mut horizontal = vec![0u8; width * height];
    let mut deque: VecDeque<usize> = VecDeque::new();

    for y in 0..height {
        deque.clear();
        let row = y * width;
        for x in 0..width {
            let add = x + radius;
            if add < width {
                let add_value = input[row + add];
                while let Some(&back) = deque.back() {
                    let back_value = input[row + back];
                    let drop = if maximum {
                        back_value <= add_value
                    } else {
                        back_value >= add_value
                    };
                    if drop {
                        deque.pop_back();
                    } else {
                        break;
                    }
                }
                deque.push_back(add);
            }
            while let Some(&front) = deque.front() {
                if front + radius < x {
                    deque.pop_front();
                } else {
                    break;
                }
            }
            horizontal[row + x] = input[row + deque.front().copied().unwrap_or(x)];
        }
    }

    let mut output = vec![0u8; width * height];
    for x in 0..width {
        deque.clear();
        for y in 0..height {
            let add = y + radius;
            if add < height {
                let add_value = horizontal[add * width + x];
                while let Some(&back) = deque.back() {
                    let back_value = horizontal[back * width + x];
                    let drop = if maximum {
                        back_value <= add_value
                    } else {
                        back_value >= add_value
                    };
                    if drop {
                        deque.pop_back();
                    } else {
                        break;
                    }
                }
                deque.push_back(add);
            }
            while let Some(&front) = deque.front() {
                if front + radius < y {
                    deque.pop_front();
                } else {
                    break;
                }
            }
            output[y * width + x] = horizontal[deque.front().copied().unwrap_or(y) * width + x];
        }
    }
    output
}

fn close_dark_ink(input: &[u8], width: usize, height: usize, radius: usize) -> Vec<u8> {
    let dilated = sliding_extreme(input, width, height, radius, true);
    sliding_extreme(&dilated, width, height, radius, false)
}

fn flatten_illumination(gray: &GrayImage) -> Vec<f32> {
    let (width, height) = gray.dimensions();
    let w = width as usize;
    let h = height as usize;
    let min_side = width.min(height) as f32;
    let radius = ((min_side * 0.045).round() as usize).clamp(8, 72);
    let paper = close_dark_ink(gray.as_raw(), w, h, radius);
    let raw = gray.as_raw();

    let mut flat = vec![255.0f32; w * h];
    for index in 0..w * h {
        let level = (paper[index] as f32).max(16.0);
        flat[index] = (raw[index] as f32 / level * 255.0).clamp(0.0, 255.0);
    }
    flat
}

fn box_gap(min_a: u32, max_a: u32, min_b: u32, max_b: u32) -> u32 {
    if max_a < min_b {
        min_b - max_a
    } else if max_b < min_a {
        min_a - max_b
    } else {
        0
    }
}

fn find_root(parent: &mut [usize], index: usize) -> usize {
    let mut root = index;
    while parent[root] != root {
        root = parent[root];
    }
    let mut current = index;
    while parent[current] != current {
        let next = parent[current];
        parent[current] = root;
        current = next;
    }
    root
}

fn union(parent: &mut [usize], a: usize, b: usize) {
    let root_a = find_root(parent, a);
    let root_b = find_root(parent, b);
    if root_a != root_b {
        parent[root_b] = root_a;
    }
}

pub fn clean_photo(path: &Path) -> Result<String, String> {
    let source = crate::image_loader::load_image_any(path)
        .map_err(|e| format!("Foto non leggibile: {e}"))?;
    let image = if source.width().max(source.height()) > MAX_INPUT_SIDE {
        source.resize(MAX_INPUT_SIDE, MAX_INPUT_SIDE, FilterType::Lanczos3)
    } else {
        source
    };
    let rgb = image.to_rgb8();
    let gray = image::imageops::grayscale(&rgb);
    let (width, height) = gray.dimensions();
    if width < 24 || height < 24 {
        return Err("La foto è troppo piccola per riconoscere una firma.".into());
    }

    let w = width as usize;
    let h = height as usize;
    let total_pixels = (width as u64) * (height as u64);
    let mean: f64 = gray.as_raw().iter().map(|value| *value as f64).sum::<f64>() / (w * h) as f64;
    if mean < 8.0 {
        return Err("La foto è troppo scura. Prova con più luce e inchiostro scuro su carta chiara.".into());
    }

    let flat = flatten_illumination(&gray);
    let mut hist = [0u64; 256];
    for value in &flat {
        hist[(*value).round().clamp(0.0, 255.0) as usize] += 1;
    }
    let threshold = otsu(&hist, total_pixels).clamp(60, 235) as f32;
    let mask: Vec<bool> = flat.iter().map(|value| *value < threshold).collect();

    let mut labels = vec![0u32; w * h];
    let mut components: Vec<Component> = Vec::new();
    let mut stack: Vec<usize> = Vec::new();
    let mut next_label = 0u32;

    for start in 0..w * h {
        if !mask[start] || labels[start] != 0 {
            continue;
        }
        next_label += 1;
        let mut area = 0u32;
        let mut min_x = w as u32;
        let mut min_y = h as u32;
        let mut max_x = 0u32;
        let mut max_y = 0u32;
        stack.push(start);
        labels[start] = next_label;

        while let Some(index) = stack.pop() {
            let x = index % w;
            let y = index / w;
            area += 1;
            min_x = min_x.min(x as u32);
            min_y = min_y.min(y as u32);
            max_x = max_x.max(x as u32);
            max_y = max_y.max(y as u32);

            let x0 = x.saturating_sub(1);
            let x1 = (x + 1).min(w - 1);
            let y0 = y.saturating_sub(1);
            let y1 = (y + 1).min(h - 1);
            for ny in y0..=y1 {
                for nx in x0..=x1 {
                    let neighbor = ny * w + nx;
                    if mask[neighbor] && labels[neighbor] == 0 {
                        labels[neighbor] = next_label;
                        stack.push(neighbor);
                    }
                }
            }
        }

        components.push(Component {
            area,
            min_x,
            min_y,
            max_x,
            max_y,
            keep: false,
        });
    }

    if components.is_empty() {
        return Err(no_signature_error());
    }

    let min_area = ((total_pixels as f64 * 0.00002).max(12.0)) as u32;
    let max_area = ((total_pixels as f64 * 0.30) as u32).max(min_area + 1);

    for component in &mut components {
        let box_width = component.max_x.saturating_sub(component.min_x) + 1;
        let box_height = component.max_y.saturating_sub(component.min_y) + 1;
        let bbox_area = (box_width as u64) * (box_height as u64);
        let fill = component.area as f64 / bbox_area.max(1) as f64;
        let touches_horizontal_edge = component.min_y == 0 || component.max_y == h as u32 - 1;
        let touches_vertical_edge = component.min_x == 0 || component.max_x == w as u32 - 1;
        let long_horizontal_border = box_width > (w as f32 * 0.60) as u32
            && box_height < (h as f32 * 0.015).max(3.0) as u32
            && touches_horizontal_edge;
        let long_vertical_border = box_height > (h as f32 * 0.60) as u32
            && box_width < (w as f32 * 0.015).max(3.0) as u32
            && touches_vertical_edge;
        let whole_image = box_width > (w as f32 * 0.94) as u32 && box_height > (h as f32 * 0.94) as u32;
        let sparse_large = box_width as f32 > w as f32 * 0.50
            && box_height as f32 > h as f32 * 0.20
            && fill < 0.035;
        let touches_any_edge = component.min_x == 0
            || component.min_y == 0
            || component.max_x == w as u32 - 1
            || component.max_y == h as u32 - 1;
        let large_edge_shadow = touches_any_edge
            && component.area as f64 > total_pixels as f64 * 0.015
            && (box_width as f32 > w as f32 * 0.30 || box_height as f32 > h as f32 * 0.30);

        component.keep = component.area >= min_area
            && component.area <= max_area
            && !long_horizontal_border
            && !long_vertical_border
            && !whole_image
            && !sparse_large
            && !large_edge_shadow;
    }

    let kept: Vec<usize> = components
        .iter()
        .enumerate()
        .filter(|(_, component)| component.keep)
        .map(|(index, _)| index)
        .collect();
    if kept.is_empty() {
        return Err(no_signature_error());
    }

    // Group nearby components and keep only the group with the most ink.
    // This removes marks that are far from the signature without chaining
    // across the whole sheet.
    let gap_limit = (width.min(height) as f32 * 0.10).max(16.0);
    let mut parent: Vec<usize> = (0..components.len()).collect();
    for a in 0..kept.len() {
        for b in (a + 1)..kept.len() {
            let first = &components[kept[a]];
            let second = &components[kept[b]];
            let dx = box_gap(first.min_x, first.max_x, second.min_x, second.max_x) as f32;
            let dy = box_gap(first.min_y, first.max_y, second.min_y, second.max_y) as f32;
            if dx.max(dy) <= gap_limit {
                union(&mut parent, kept[a], kept[b]);
            }
        }
    }

    let mut totals: HashMap<usize, u64> = HashMap::new();
    for &index in &kept {
        let root = find_root(&mut parent, index);
        *totals.entry(root).or_insert(0) += components[index].area as u64;
    }
    let best_root = match totals.iter().max_by_key(|(_, total)| **total) {
        Some((root, _)) => *root,
        None => return Err(no_signature_error()),
    };

    for &index in &kept {
        components[index].keep = find_root(&mut parent, index) == best_root;
    }

    let mut min_x = width;
    let mut min_y = height;
    let mut max_x = 0u32;
    let mut max_y = 0u32;
    let mut selected_area = 0u64;
    for component in &components {
        if !component.keep {
            continue;
        }
        min_x = min_x.min(component.min_x);
        min_y = min_y.min(component.min_y);
        max_x = max_x.max(component.max_x);
        max_y = max_y.max(component.max_y);
        selected_area += component.area as u64;
    }
    if selected_area == 0 {
        return Err(no_signature_error());
    }

    let min_total = ((total_pixels as f64 * 0.00012).max(48.0)) as u64;
    if selected_area < min_total {
        return Err(no_signature_error());
    }

    let padding = (((max_x - min_x + 1).max(max_y - min_y + 1)) as f32 * 0.06).max(4.0) as u32;
    let crop_min_x = min_x.saturating_sub(padding);
    let crop_min_y = min_y.saturating_sub(padding);
    let crop_max_x = (max_x + padding).min(width - 1);
    let crop_max_y = (max_y + padding).min(height - 1);
    let crop_width = crop_max_x.saturating_sub(crop_min_x) + 1;
    let crop_height = crop_max_y.saturating_sub(crop_min_y) + 1;

    let mut output = RgbaImage::from_pixel(crop_width, crop_height, Rgba([INK[0], INK[1], INK[2], 0]));
    let soft = (threshold * 0.40).max(1.0);
    for y in crop_min_y..=crop_max_y {
        for x in crop_min_x..=crop_max_x {
            let source_index = (y as usize) * w + x as usize;
            let label = labels[source_index];
            if label == 0 {
                continue;
            }
            let component = &components[(label - 1) as usize];
            if !component.keep {
                continue;
            }
            let alpha = ((threshold - flat[source_index]) / soft).clamp(0.0, 1.0);
            let alpha_byte = (alpha * 255.0).clamp(24.0, 255.0) as u8;
            if alpha_byte > 0 {
                output.put_pixel(
                    x - crop_min_x,
                    y - crop_min_y,
                    Rgba([INK[0], INK[1], INK[2], alpha_byte]),
                );
            }
        }
    }

    let output = if crop_width.max(crop_height) > MAX_OUTPUT_SIDE {
        let scale = MAX_OUTPUT_SIDE as f32 / crop_width.max(crop_height) as f32;
        let new_width = ((crop_width as f32 * scale).round() as u32).max(1);
        let new_height = ((crop_height as f32 * scale).round() as u32).max(1);
        image::DynamicImage::ImageRgba8(output).resize(new_width, new_height, FilterType::Lanczos3)
    } else {
        image::DynamicImage::ImageRgba8(output)
    };

    let mut bytes = Cursor::new(Vec::new());
    output
        .write_to(&mut bytes, image::ImageFormat::Png)
        .map_err(|e| format!("Preparazione della firma non riuscita: {e}"))?;
    Ok(format!("data:image/png;base64,{}", B64.encode(bytes.into_inner())))
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::Luma;

    fn decode(data: &str) -> RgbaImage {
        assert!(data.starts_with("data:image/png;base64,"));
        let raw = B64
            .decode(data.trim_start_matches("data:image/png;base64,"))
            .unwrap();
        image::load_from_memory(&raw).unwrap().to_rgba8()
    }

    fn draw_stroke(image: &mut GrayImage, x0: u32, x1: u32, y_at: impl Fn(u32) -> i32, half: i32, value: u8) {
        let height = image.height() as i32;
        for x in x0..x1 {
            let y = y_at(x);
            for dy in -half..=half {
                let py = (y + dy).clamp(0, height - 1) as u32;
                image.put_pixel(x, py, Luma([value]));
            }
        }
    }

    fn make_photo(path: &Path) {
        let width = 640u32;
        let height = 320u32;
        let mut image = GrayImage::new(width, height);
        for y in 0..height {
            for x in 0..width {
                let shade = 118.0 + (x as f32 / width as f32) * 92.0;
                let noise = (((x * 13 + y * 7) % 11) as f32) - 5.0;
                image.put_pixel(x, y, Luma([(shade + noise).clamp(0.0, 255.0) as u8]));
            }
        }
        draw_stroke(
            &mut image,
            120,
            520,
            |x| 170 + ((x as f32 * 0.06).sin() * 38.0) as i32,
            3,
            18,
        );
        for i in 0..35u32 {
            let x = (i * 47 + 23) % width;
            let y = (i * 31 + 17) % height;
            if x > 80 && x < 560 && y > 90 && y < 240 {
                continue;
            }
            image.put_pixel(x, y, Luma([40]));
        }
        image.save(path).unwrap();
    }

    #[test]
    fn cleans_signature_photo() {
        let path = std::env::temp_dir().join("openpdf-signature-test.png");
        make_photo(&path);
        let image = decode(&clean_photo(&path).unwrap());
        assert!(image.width() > 40);
        assert!(image.height() > 20);
        let alpha = image.pixels().filter(|pixel| pixel[3] > 0).count();
        assert!(alpha > 100, "firma non rilevata: {alpha} pixel");
        assert!(image.width() < 640, "la firma non è stata ritagliata");
        assert_eq!(image.get_pixel(0, 0)[3], 0, "lo sfondo deve essere trasparente");
        let dark = image
            .pixels()
            .filter(|pixel| pixel[3] > 200 && pixel[0] < 60 && pixel[1] < 60)
            .count();
        assert!(dark > 100, "inchiostro scuro non trovato: {dark} pixel");
    }

    #[test]
    fn removes_far_marks_in_shadowed_photo() {
        let width = 700u32;
        let height = 360u32;
        let mut image = GrayImage::new(width, height);
        for y in 0..height {
            for x in 0..width {
                let base = if x < width / 2 { 118.0 } else { 206.0 };
                let shade = base + (y as f32 / height as f32) * 20.0;
                let noise = (((x * 11 + y * 5) % 9) as f32) - 4.0;
                image.put_pixel(x, y, Luma([(shade + noise).clamp(0.0, 255.0) as u8]));
            }
        }
        draw_stroke(
            &mut image,
            90,
            390,
            |x| 200 + ((x as f32 * 0.05).sin() * 24.0) as i32,
            2,
            15,
        );
        for y in 40..70 {
            for x in 620..650 {
                image.put_pixel(x, y, Luma([24]));
            }
        }
        let path = std::env::temp_dir().join("openpdf-signature-shadow-test.png");
        image.save(&path).unwrap();

        let cleaned = decode(&clean_photo(&path).unwrap());
        assert!(cleaned.width() < 500, "il segno lontano non è stato rimosso");
        let alpha = cleaned.pixels().filter(|pixel| pixel[3] > 0).count();
        assert!(alpha > 100, "firma non rilevata: {alpha} pixel");
    }

    #[test]
    fn ignores_large_edge_shadows() {
        let width = 760u32;
        let height = 380u32;
        let mut image = GrayImage::new(width, height);
        for y in 0..height {
            for x in 0..width {
                let shade = 172.0 + (x as f32 / width as f32) * 46.0;
                let noise = (((x * 17 + y * 9) % 9) as f32) - 4.0;
                let value = if x < 26 {
                    52.0 + (((x * 5 + y * 3) % 7) as f32)
                } else {
                    shade + noise
                };
                image.put_pixel(x, y, Luma([value.clamp(0.0, 255.0) as u8]));
            }
        }
        draw_stroke(
            &mut image,
            170,
            580,
            |x| 215 + ((x as f32 * 0.06).sin() * 30.0) as i32,
            3,
            16,
        );
        let path = std::env::temp_dir().join("openpdf-signature-edge-test.png");
        image.save(&path).unwrap();

        let cleaned = decode(&clean_photo(&path).unwrap());
        assert!(cleaned.width() < 520, "l'ombra sul bordo non è stata rimossa");
        assert!(cleaned.width() > 200, "la firma è stata persa");
        assert!(cleaned.height() < 220, "il ritaglio è troppo alto");
        let alpha = cleaned.pixels().filter(|pixel| pixel[3] > 0).count();
        assert!(alpha > 100, "firma non rilevata: {alpha} pixel");
    }

    #[test]
    fn rejects_blank_photo() {
        let width = 480u32;
        let height = 240u32;
        let mut image = GrayImage::new(width, height);
        for y in 0..height {
            for x in 0..width {
                let noise = (((x * 7 + y * 3) % 5) as i32) - 2;
                image.put_pixel(x, y, Luma([(205 + noise) as u8]));
            }
        }
        let path = std::env::temp_dir().join("openpdf-signature-blank-test.png");
        image.save(&path).unwrap();
        let error = clean_photo(&path).unwrap_err();
        assert!(error.contains("Nessuna firma"), "errore inatteso: {error}");
    }
}
