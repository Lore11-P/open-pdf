use crate::engine;
use crate::state::app_data_dir;
use base64::Engine as _;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Child;

const OCR_SCRIPT: &str = r#"
$ErrorActionPreference='Stop'
Add-Type -AssemblyName System.Runtime.WindowsRuntime
$asTaskGeneric = ([System.WindowsRuntimeSystemExtensions].GetMethods() | Where-Object { $_.Name -eq 'AsTask' -and $_.GetParameters().Count -eq 1 -and $_.GetParameters()[0].ParameterType.Name -eq 'IAsyncOperation`1' })[0]
function Await($WinRtTask, $ResultType) {
  $asTask = $asTaskGeneric.MakeGenericMethod($ResultType)
  $netTask = $asTask.Invoke($null, @($WinRtTask))
  $netTask.Wait(-1) | Out-Null
  $netTask.Result
}
[Windows.Media.Ocr.OcrEngine, Windows.Foundation, ContentType=WindowsRuntime] | Out-Null
[Windows.Graphics.Imaging.BitmapDecoder, Windows.Foundation, ContentType=WindowsRuntime] | Out-Null
[Windows.Storage.StorageFile, Windows.Foundation, ContentType=WindowsRuntime] | Out-Null
$file = Await ([Windows.Storage.StorageFile]::GetFileFromPathAsync($args[0])) ([Windows.Storage.StorageFile])
$stream = Await ($file.OpenAsync([Windows.Storage.FileAccessMode]::Read)) ([Windows.Storage.Streams.IRandomAccessStream])
$decoder = Await ([Windows.Graphics.Imaging.BitmapDecoder]::CreateAsync($stream)) ([Windows.Graphics.Imaging.BitmapDecoder])
$bitmap = Await ($decoder.GetSoftwareBitmapAsync()) ([Windows.Graphics.Imaging.SoftwareBitmap])
$engine = [Windows.Media.Ocr.OcrEngine]::TryCreateFromUserProfileLanguages()
if ($null -eq $engine) { Write-Error 'Motore OCR di Windows non disponibile'; exit 2 }
$result = Await ($engine.RecognizeAsync($bitmap)) ([Windows.Media.Ocr.OcrResult])
$result.Text
"#;

const TTS_SCRIPT: &str = r#"
$ErrorActionPreference='SilentlyContinue'
Add-Type -AssemblyName System.Speech
$sp = New-Object System.Speech.Synthesis.SpeechSynthesizer
$sp.Rate = [int]$args[1]
try { $sp.SelectVoiceByHints([System.Speech.Synthesis.VoiceGender]::NotSet, [System.Speech.Synthesis.VoiceAge]::NotSet, 0, [System.Globalization.CultureInfo]::GetCultureInfo('it-IT')) } catch { }
$text = [System.IO.File]::ReadAllText($args[0], [System.Text.Encoding]::UTF8)
$sp.Speak($text)
"#;

fn script_path(name: &str, content: &str) -> Result<PathBuf, String> {
    let dir = app_data_dir().join("scripts");
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let path = dir.join(name);
    std::fs::write(&path, content).map_err(|e| e.to_string())?;
    Ok(path)
}

pub fn ocr_image(path: &Path) -> Result<String, String> {
    let script = script_path("ocr.ps1", OCR_SCRIPT)?;
    let output = std::process::Command::new("powershell.exe")
        .args([
            "-NoProfile",
            "-ExecutionPolicy",
            "Bypass",
            "-File",
            &script.to_string_lossy(),
            &path.to_string_lossy(),
        ])
        .output()
        .map_err(|e| format!("OCR non avviato: {e}"))?;
    if !output.status.success() {
        return Err(format!(
            "OCR di Windows non riuscito: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    let text = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if text.is_empty() {
        return Err("Nessun testo riconosciuto nell'immagine.".into());
    }
    Ok(text)
}

pub fn ocr_pdf_page(path: &Path, page: usize, password: Option<&str>) -> Result<String, String> {
    let result = engine::render_page_png(path, page, 2300, password)?;
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(result.png)
        .map_err(|e| e.to_string())?;
    let dir = app_data_dir().join("ocr");
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let temp = dir.join(format!(
        "page-{}-{}.png",
        page + 1,
        &uuid::Uuid::new_v4().to_string()[..8]
    ));
    std::fs::write(&temp, bytes).map_err(|e| e.to_string())?;
    let text = ocr_image(&temp);
    let _ = std::fs::remove_file(&temp);
    text
}

pub fn speak(text: &str, rate: i32) -> Result<Child, String> {
    if text.trim().is_empty() {
        return Err("Nessun testo da leggere nella pagina.".into());
    }
    let script = script_path("tts.ps1", TTS_SCRIPT)?;
    let dir = app_data_dir().join("tts");
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let txt = dir.join(format!("speech-{}.txt", &uuid::Uuid::new_v4().to_string()[..8]));
    {
        let mut file = std::fs::File::create(&txt).map_err(|e| e.to_string())?;
        file.write_all(text.as_bytes()).map_err(|e| e.to_string())?;
    }
    let child = std::process::Command::new("powershell.exe")
        .args([
            "-NoProfile",
            "-ExecutionPolicy",
            "Bypass",
            "-File",
            &script.to_string_lossy(),
            &txt.to_string_lossy(),
            &rate.clamp(-6, 6).to_string(),
        ])
        .spawn()
        .map_err(|e| format!("Lettura vocale non avviata: {e}"))?;
    Ok(child)
}

pub fn stop(child: Option<Child>) {
    if let Some(mut c) = child {
        let _ = c.kill();
        let _ = c.wait();
    }
}


const OCR_LINES_SCRIPT: &str = r#"
$ErrorActionPreference='Stop'
Add-Type -AssemblyName System.Runtime.WindowsRuntime
$asTaskGeneric = ([System.WindowsRuntimeSystemExtensions].GetMethods() | Where-Object { $_.Name -eq 'AsTask' -and $_.GetParameters().Count -eq 1 -and $_.GetParameters()[0].ParameterType.Name -eq 'IAsyncOperation`1' })[0]
function Await($WinRtTask, $ResultType) {
  $asTask = $asTaskGeneric.MakeGenericMethod($ResultType)
  $netTask = $asTask.Invoke($null, @($WinRtTask))
  $netTask.Wait(-1) | Out-Null
  $netTask.Result
}
[Windows.Media.Ocr.OcrEngine, Windows.Foundation, ContentType=WindowsRuntime] | Out-Null
[Windows.Graphics.Imaging.BitmapDecoder, Windows.Foundation, ContentType=WindowsRuntime] | Out-Null
[Windows.Storage.StorageFile, Windows.Foundation, ContentType=WindowsRuntime] | Out-Null
$file = Await ([Windows.Storage.StorageFile]::GetFileFromPathAsync($args[0])) ([Windows.Storage.StorageFile])
$stream = Await ($file.OpenAsync([Windows.Storage.FileAccessMode]::Read)) ([Windows.Storage.Streams.IRandomAccessStream])
$decoder = Await ([Windows.Graphics.Imaging.BitmapDecoder]::CreateAsync($stream)) ([Windows.Graphics.Imaging.BitmapDecoder])
$bitmap = Await ($decoder.GetSoftwareBitmapAsync()) ([Windows.Graphics.Imaging.SoftwareBitmap])
$engine = [Windows.Media.Ocr.OcrEngine]::TryCreateFromUserProfileLanguages()
if ($null -eq $engine) { Write-Error 'Motore OCR di Windows non disponibile'; exit 2 }
$result = Await ($engine.RecognizeAsync($bitmap)) ([Windows.Media.Ocr.OcrResult])
$out = @()
foreach ($line in $result.Lines) {
  $ws = @($line.Words)
  if ($ws.Count -eq 0) { continue }
  $minX = ($ws | ForEach-Object { $_.BoundingRect.X } | Measure-Object -Minimum).Minimum
  $minY = ($ws | ForEach-Object { $_.BoundingRect.Y } | Measure-Object -Minimum).Minimum
  $maxX = ($ws | ForEach-Object { $_.BoundingRect.X + $_.BoundingRect.Width } | Measure-Object -Maximum).Maximum
  $maxY = ($ws | ForEach-Object { $_.BoundingRect.Y + $_.BoundingRect.Height } | Measure-Object -Maximum).Maximum
  $out += [PSCustomObject]@{ text = $line.Text; x = [double]$minX; y = [double]$minY; w = [double]($maxX - $minX); h = [double]($maxY - $minY) }
}
ConvertTo-Json -InputObject @($out) -Compress -Depth 4
"#;

#[derive(serde::Deserialize)]
struct OcrLine {
    text: String,
    x: f32,
    y: f32,
    w: f32,
    h: f32,
}

fn ocr_lines_image(path: &Path) -> Result<Vec<OcrLine>, String> {
    let script = script_path("ocr-lines.ps1", OCR_LINES_SCRIPT)?;
    let output = std::process::Command::new("powershell.exe")
        .args([
            "-NoProfile",
            "-ExecutionPolicy",
            "Bypass",
            "-File",
            &script.to_string_lossy(),
            &path.to_string_lossy(),
        ])
        .output()
        .map_err(|e| format!("OCR non avviato: {e}"))?;
    if !output.status.success() {
        return Err(format!(
            "OCR di Windows non riuscito: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    let raw = String::from_utf8_lossy(&output.stdout);
    let raw = raw.trim();
    if raw.is_empty() {
        return Ok(vec![]);
    }
    let value: serde_json::Value = serde_json::from_str(raw)
        .map_err(|e| format!("Risposta OCR non valida: {e}"))?;
    let items = match value {
        serde_json::Value::Array(items) => items,
        obj @ serde_json::Value::Object(_) => vec![obj],
        _ => vec![],
    };
    let mut lines = Vec::new();
    for item in items {
        if let Ok(line) = serde_json::from_value::<OcrLine>(item) {
            if !line.text.trim().is_empty() {
                lines.push(line);
            }
        }
    }
    Ok(lines)
}

fn add_invisible_line<'a>(
    doc: &pdfium_render::prelude::PdfDocument<'a>,
    page: &mut pdfium_render::prelude::PdfPage<'a>,
    token: pdfium_render::prelude::PdfFontToken,
    text: &str,
    x: f32,
    y: f32,
    size: f32,
    rotation: i32,
) -> Result<(), String> {
    use pdfium_render::prelude::*;
    let mut obj = PdfPageTextObject::new(doc, text, token, PdfPoints::new(size.max(3.0)))
        .map_err(|e| e.to_string())?;
    obj.set_render_mode(PdfPageTextRenderMode::Invisible)
        .map_err(|e| e.to_string())?;
    if rotation != 0 {
        obj.rotate_counter_clockwise_degrees(rotation as f32)
            .map_err(|e| e.to_string())?;
    }
    obj.translate(PdfPoints::new(x), PdfPoints::new(y))
        .map_err(|e| e.to_string())?;
    page.objects_mut().add_text_object(obj).map_err(|e| e.to_string())?;
    Ok(())
}

pub fn make_searchable(
    src: &Path,
    dest: &Path,
    password: Option<&str>,
    from: usize,
    to: usize,
) -> Result<usize, String> {
    let mut doc = engine::open_doc(src, password)?;
    let token = doc.fonts_mut().new_built_in(pdfium_render::prelude::PdfFontBuiltin::Helvetica);
    let count = doc.pages().len() as usize;
    let start = from.min(count.saturating_sub(1));
    let end = to.min(count.saturating_sub(1));
    let mut pages_done = 0usize;
    let dir = app_data_dir().join("ocr");
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    for i in start..=end {
        let space = {
            let page = doc.pages().get(i as i32).map_err(|e| e.to_string())?;
            engine::page_space(&page)?
        };
        let img = engine::render_page_image(&doc, i, 2000)?;
        let (img_w, img_h) = (img.width() as f32, img.height() as f32);
        let temp = dir.join(format!("layer-{}-{}.png", i + 1, &uuid::Uuid::new_v4().to_string()[..8]));
        let rgb = img.to_rgb8();
        rgb.save(&temp).map_err(|e| e.to_string())?;
        let lines = match ocr_lines_image(&temp) {
            Ok(lines) => lines,
            Err(e) => {
                let _ = std::fs::remove_file(&temp);
                return Err(e);
            }
        };
        let _ = std::fs::remove_file(&temp);
        if lines.is_empty() {
            continue;
        }
        {
            let mut page = doc.pages_mut().get(i as i32).map_err(|e| e.to_string())?;
            for line in lines {
                let nx = (line.x / img_w).clamp(0.0, 1.0);
                let ny = (line.y / img_h).clamp(0.0, 1.0);
                let nh = (line.h / img_h).clamp(0.001, 0.2);
                let (x, y) = crate::annotate::norm_to_page(&space, nx, ny + nh * 0.82);
                let size = (nh * space.display_height * 0.9).clamp(3.0, 48.0);
                add_invisible_line(&doc, &mut page, token, &line.text, x, y, size, space.rotation)?;
            }
            page.regenerate_content().map_err(|e| e.to_string())?;
        }
        pages_done += 1;
    }
    doc.save_to_file(dest).map_err(|e| e.to_string())?;
    Ok(pages_done)
}
