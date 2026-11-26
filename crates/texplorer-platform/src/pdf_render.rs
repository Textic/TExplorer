//! Native Windows PDF page renderer powered by WinRT `Windows.Data.Pdf`.
//!
//! Uses the built-in Windows 10/11 PDF engine with zero external DLLs or third-party C++ libraries.
//! Renders any page of a local, archive-extracted, or remote PDF into a crisp high-DPI PNG image.

/// A single rendered PDF page returned by `Windows.Data.Pdf`.
#[derive(Debug, Clone)]
pub struct RenderedPdfPage {
    /// Zero-based page index that was rendered (clamped to `0..page_count`).
    pub page_index: u32,
    /// Total number of pages in the PDF document.
    pub page_count: u32,
    /// Rendered pixel width.
    pub width: u32,
    /// Rendered pixel height.
    pub height: u32,
    /// Encoded PNG image bytes representing the rendered page on a white background.
    pub png_bytes: Vec<u8>,
}

/// Fits a PDF page's point/DIP dimensions proportionally so its longest edge equals `max_dim`,
/// keeping crisp readable output on both standard and HiDPI displays.
pub fn fit_pdf_dimensions(page_w: f32, page_h: f32, max_dim: u32) -> (u32, u32) {
    let max_d = max_dim.max(64) as f32;
    if !page_w.is_finite() || !page_h.is_finite() || page_w <= 0.0 || page_h <= 0.0 {
        return (max_dim.max(64), max_dim.max(64));
    }
    let scale = (max_d / page_w).min(max_d / page_h);
    let w = (page_w * scale).round().max(1.0) as u32;
    let h = (page_h * scale).round().max(1.0) as u32;
    (w, h)
}

#[cfg(target_os = "windows")]
mod imp {
    use std::path::Path;

    use windows::Data::Pdf::{PdfDocument, PdfPageRenderOptions};
    use windows::Storage::StorageFile;
    use windows::Storage::Streams::{DataReader, DataWriter, InMemoryRandomAccessStream};
    use windows::UI::Color;
    use windows::Win32::Foundation::RPC_E_CHANGED_MODE;
    use windows::Win32::System::WinRT::{RO_INIT_MULTITHREADED, RoInitialize, RoUninitialize};
    use windows_core::HSTRING;

    use super::{RenderedPdfPage, fit_pdf_dimensions};

    const ERROR_INVALID_PASSWORD_HR: i32 = 0x8007_0056_u32 as i32;
    const MAX_STREAM_FALLBACK_BYTES: u64 = 64 * 1024 * 1024;

    struct RoMtaGuard;
    impl Drop for RoMtaGuard {
        fn drop(&mut self) {
            unsafe {
                RoUninitialize();
            }
        }
    }

    /// Executes `f` inside a WinRT Multi-Threaded Apartment (MTA) so blocking `.get()` calls on
    /// `IAsyncOperation` / `IAsyncAction` never deadlock on an unpumped STA thread.
    fn run_in_winrt_mta<F, T>(f: F) -> Result<T, String>
    where
        F: FnOnce() -> Result<T, String> + Send + 'static,
        T: Send + 'static,
    {
        let init = unsafe { RoInitialize(RO_INIT_MULTITHREADED) };
        match init {
            Ok(()) => {
                let _guard = RoMtaGuard;
                f()
            }
            Err(err) if err.code() == RPC_E_CHANGED_MODE => {
                std::thread::Builder::new()
                    .name("texplorer-pdf-winrt".to_string())
                    .spawn(move || {
                        unsafe {
                            RoInitialize(RO_INIT_MULTITHREADED)
                                .map_err(|e| format!("Failed initializing WinRT PDF engine: {e}"))?;
                        }
                        let _guard = RoMtaGuard;
                        f()
                    })
                    .map_err(|e| format!("Failed spawning WinRT PDF worker thread: {e}"))?
                    .join()
                    .map_err(|_| "WinRT PDF worker thread panicked.".to_string())?
            }
            Err(err) => Err(format!("Failed initializing WinRT PDF engine: {err}")),
        }
    }

    fn format_pdf_open_error(err: windows_core::Error) -> String {
        if err.code().0 == ERROR_INVALID_PASSWORD_HR {
            return "Password-protected PDF — click Open to view in your PDF reader.".to_string();
        }
        let msg = err.to_string();
        let lower = msg.to_ascii_lowercase();
        if lower.contains("password") || lower.contains("80070056") {
            "Password-protected PDF — click Open to view in your PDF reader.".to_string()
        } else {
            format!("Unable to render PDF preview: {msg}")
        }
    }

    fn clean_winrt_path(path: &Path) -> String {
        let abs = if path.is_absolute() {
            path.to_path_buf()
        } else {
            std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
        };
        let raw = abs.to_string_lossy();
        if let Some(unc) = raw.strip_prefix(r"\\?\UNC\") {
            format!(r"\\{unc}")
        } else if let Some(stripped) = raw.strip_prefix(r"\\?\") {
            stripped.to_string()
        } else {
            raw.into_owned()
        }
    }

    fn load_document_from_bytes(bytes: &[u8]) -> Result<PdfDocument, String> {
        let stream = InMemoryRandomAccessStream::new()
            .map_err(|e| format!("Failed creating PDF memory stream: {e}"))?;
        let output = stream
            .GetOutputStreamAt(0)
            .map_err(|e| format!("Failed opening PDF memory output stream: {e}"))?;
        let writer = DataWriter::CreateDataWriter(&output)
            .map_err(|e| format!("Failed creating PDF stream writer: {e}"))?;
        writer
            .WriteBytes(bytes)
            .map_err(|e| format!("Failed writing PDF bytes: {e}"))?;
        writer
            .StoreAsync()
            .and_then(|op| op.get())
            .map_err(|e| format!("Failed storing PDF stream bytes: {e}"))?;
        writer
            .FlushAsync()
            .and_then(|op| op.get())
            .map_err(|e| format!("Failed flushing PDF stream bytes: {e}"))?;
        let _ = writer.DetachStream();
        stream
            .Seek(0)
            .map_err(|e| format!("Failed seeking PDF stream: {e}"))?;

        PdfDocument::LoadFromStreamAsync(&stream)
            .and_then(|op| op.get())
            .map_err(format_pdf_open_error)
    }

    fn render_document_page(
        doc: &PdfDocument,
        page_index: u32,
        max_dim: u32,
    ) -> Result<RenderedPdfPage, String> {
        let page_count = doc
            .PageCount()
            .map_err(|e| format!("Failed reading PDF page count: {e}"))?;
        if page_count == 0 {
            return Err("PDF document contains no pages.".to_string());
        }

        let clamped_index = page_index.min(page_count - 1);
        let page = doc
            .GetPage(clamped_index)
            .map_err(|e| format!("Failed opening PDF page {}: {e}", clamped_index + 1))?;
        let size = page
            .Size()
            .map_err(|e| format!("Failed reading PDF page dimensions: {e}"))?;
        let (target_w, target_h) = fit_pdf_dimensions(size.Width, size.Height, max_dim);

        let options = PdfPageRenderOptions::new()
            .map_err(|e| format!("Failed creating PDF render options: {e}"))?;
        options
            .SetDestinationWidth(target_w)
            .map_err(|e| format!("Failed setting PDF render width: {e}"))?;
        options
            .SetDestinationHeight(target_h)
            .map_err(|e| format!("Failed setting PDF render height: {e}"))?;
        options
            .SetBackgroundColor(Color {
                A: 255,
                R: 255,
                G: 255,
                B: 255,
            })
            .map_err(|e| format!("Failed setting PDF background color: {e}"))?;
        let _ = options.SetIsIgnoringHighContrast(true);

        let out_stream = InMemoryRandomAccessStream::new()
            .map_err(|e| format!("Failed creating PDF render output stream: {e}"))?;
        page.RenderWithOptionsToStreamAsync(&out_stream, &options)
            .and_then(|op| op.get())
            .map_err(|e| format!("Failed rendering PDF page {}: {e}", clamped_index + 1))?;
        let _ = page.Close();

        let stream_size = out_stream
            .Size()
            .map_err(|e| format!("Failed reading rendered PDF stream size: {e}"))?;
        if stream_size == 0 || stream_size > u32::MAX as u64 {
            return Err("Rendered PDF page image was empty.".to_string());
        }
        let byte_len = stream_size as u32;
        let input = out_stream
            .GetInputStreamAt(0)
            .map_err(|e| format!("Failed opening rendered PDF stream: {e}"))?;
        let reader = DataReader::CreateDataReader(&input)
            .map_err(|e| format!("Failed creating PDF image reader: {e}"))?;
        let loaded = reader
            .LoadAsync(byte_len)
            .and_then(|op| op.get())
            .map_err(|e| format!("Failed loading rendered PDF image bytes: {e}"))?;
        let mut png_bytes = vec![0u8; loaded as usize];
        reader
            .ReadBytes(&mut png_bytes)
            .map_err(|e| format!("Failed reading rendered PDF image bytes: {e}"))?;
        let _ = reader.DetachStream();

        Ok(RenderedPdfPage {
            page_index: clamped_index,
            page_count,
            width: target_w,
            height: target_h,
            png_bytes,
        })
    }

    pub fn render_pdf_page_from_path(
        path: &Path,
        page_index: u32,
        max_dim: u32,
    ) -> Result<RenderedPdfPage, String> {
        let path_owned = path.to_path_buf();
        run_in_winrt_mta(move || {
            let clean = clean_winrt_path(&path_owned);
            let hpath = HSTRING::from(&clean);
            let doc = match StorageFile::GetFileFromPathAsync(&hpath).and_then(|op| op.get()) {
                Ok(file) => PdfDocument::LoadFromFileAsync(&file)
                    .and_then(|op| op.get())
                    .map_err(format_pdf_open_error)?,
                Err(_) => {
                    let meta = std::fs::metadata(&path_owned)
                        .map_err(|e| format!("Unable to open PDF file: {e}"))?;
                    if meta.len() > MAX_STREAM_FALLBACK_BYTES {
                        return Err(
                            "PDF is too large for memory-stream fallback; click Open to view natively."
                                .to_string(),
                        );
                    }
                    let bytes = std::fs::read(&path_owned)
                        .map_err(|e| format!("Unable to read PDF file: {e}"))?;
                    load_document_from_bytes(&bytes)?
                }
            };
            render_document_page(&doc, page_index, max_dim)
        })
    }

    pub fn render_pdf_page_from_bytes(
        bytes: &[u8],
        page_index: u32,
        max_dim: u32,
    ) -> Result<RenderedPdfPage, String> {
        let owned = bytes.to_vec();
        run_in_winrt_mta(move || {
            let doc = load_document_from_bytes(&owned)?;
            render_document_page(&doc, page_index, max_dim)
        })
    }
}

#[cfg(not(target_os = "windows"))]
mod imp {
    use std::path::Path;

    use super::RenderedPdfPage;

    pub fn render_pdf_page_from_path(
        _path: &Path,
        _page_index: u32,
        _max_dim: u32,
    ) -> Result<RenderedPdfPage, String> {
        Err("Native PDF preview is currently supported on Windows only.".to_string())
    }

    pub fn render_pdf_page_from_bytes(
        _bytes: &[u8],
        _page_index: u32,
        _max_dim: u32,
    ) -> Result<RenderedPdfPage, String> {
        Err("Native PDF preview is currently supported on Windows only.".to_string())
    }
}

pub use imp::{render_pdf_page_from_bytes, render_pdf_page_from_path};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fits_pdf_dimensions_preserving_aspect_ratio() {
        // US Letter (612 x 792) scaled to max_dim 1400
        let (w, h) = fit_pdf_dimensions(612.0, 792.0, 1400);
        assert_eq!(h, 1400);
        assert_eq!(w, 1082);

        // Landscape A4 (842 x 595)
        let (lw, lh) = fit_pdf_dimensions(842.0, 595.0, 1200);
        assert_eq!(lw, 1200);
        assert_eq!(lh, 848);

        // Guard invalid dimensions
        assert_eq!(fit_pdf_dimensions(0.0, f32::NAN, 800), (800, 800));
    }
}
