//! Headless-Chromium export backend (chromiumoxide via browser-use).
//!
//! Flow mirrors slidev's `export.ts`: navigate the loopback print page
//! (one-time token, no session), poll the wait protocol
//! (`deckReady` + drained `[data-waitfor]`), then capture —
//!
//! - **PDF**: `mode=pdf` stacks every slide at exact page height;
//!   `Page.printToPDF` with `@page` size paginates one slide per page
//!   (slidev's one-piece mode, no post-merge needed).
//! - **PNG**: `__deckGoto(slide, click)` per page, `Emulation
//!   .setDeviceMetricsOverride` for DPI, clip-screenshot at canvas size,
//!   zipped into one artifact.

use std::time::Duration;

use chromiumoxide::cdp::browser_protocol::emulation::SetDeviceMetricsOverrideParams;
use chromiumoxide::cdp::browser_protocol::page::PrintToPdfParams;
use chromiumoxide::page::ScreenshotParams;
use chromiumoxide::cdp::browser_protocol::page::CaptureScreenshotFormat;
use serde_json::Value;

use crate::error::{Error, Result};
use crate::export::{ExportJob, ExportOutput};
use crate::store::ExportFormat;

use browser_use::{Browser, BrowserOptions, Tab};

/// Wait-protocol budget per page.
const READY_TIMEOUT: Duration = Duration::from_secs(60);
const READY_POLL: Duration = Duration::from_millis(500);

/// Headless-Chromium exporter bound to the daemon's loopback origin.
pub struct ChromiumExporter {
    /// e.g. `http://127.0.0.1:8765` (the gateway's own API addr).
    pub origin: String,
    pub options: BrowserOptions,
}

impl ChromiumExporter {
    pub fn new(origin: impl Into<String>) -> Self {
        Self {
            origin: origin.into(),
            options: BrowserOptions {
                headless: true,
                ..Default::default()
            },
        }
    }

    fn print_url(&self, deck_id: &str, token: &str, mode: &str, clicks: Option<&str>) -> String {
        let mut url = format!(
            "{}/deck/print?deck={}&token={}&mode={mode}",
            self.origin.trim_end_matches('/'),
            urlencode(deck_id),
            urlencode(token),
        );
        if let Some(c) = clicks {
            url.push_str(&format!("&clicks={c}"));
        }
        url
    }
}

fn urlencode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~') {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// Poll `deckReady` + `[data-waitfor]` drain (slidev export protocol).
async fn wait_ready(tab: &Tab) -> Result<()> {
    let deadline = tokio::time::Instant::now() + READY_TIMEOUT;
    loop {
        let value = tab
            .evaluate(
                "(() => ({ ready: document.documentElement.dataset.deckReady === 'true', \
                 waitfor: document.querySelectorAll('[data-waitfor]').length }))()",
            )
            .await
            .map_err(|e| Error::Export(format!("evaluate failed: {e}")))?;
        let (ready, waitfor) = parse_ready(&value);
        if ready && waitfor == 0 {
            return Ok(());
        }
        if tokio::time::Instant::now() >= deadline {
            return Err(Error::Export(format!(
                "print page not ready within {READY_TIMEOUT:?} (ready={ready}, waitfor={waitfor})"
            )));
        }
        tokio::time::sleep(READY_POLL).await;
    }
}

fn parse_ready(value: &Value) -> (bool, u64) {
    let ready = value
        .get("ready")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let waitfor = value
        .get("waitfor")
        .and_then(Value::as_u64)
        .unwrap_or(u64::MAX);
    (ready, waitfor)
}

#[async_trait::async_trait]
impl crate::export::DeckExporter for ChromiumExporter {
    fn name(&self) -> &'static str {
        "chromium"
    }

    async fn export(&self, job: ExportJob) -> Result<ExportOutput> {
        if job.print_token.is_empty() {
            return Err(Error::Export("export job carries no print token".into()));
        }
        let (canvas_w, canvas_h) = (job.canvas_width, job.canvas_height);

        let mut browser = Browser::launch(self.options.clone())
            .await
            .map_err(|e| Error::Export(format!("browser launch failed: {e}")))?;
        let export_result = self.run_job(&browser, &job, canvas_w, canvas_h).await;
        let _ = browser.close().await;
        export_result
    }

    async fn preview_png(&self, job: &ExportJob, slide_no: u32, clicks: u32) -> Result<Vec<u8>> {
        ChromiumExporter::preview_png(self, job, slide_no, clicks).await
    }
}

impl ChromiumExporter {
    /// Single-slide synchronous preview: open the print page, DPI-scale,
    /// goto (slide, clicks), wait, clip-screenshot. No queue, no artifact.
    pub async fn preview_png(
        &self,
        job: &ExportJob,
        slide_no: u32,
        clicks: u32,
    ) -> Result<Vec<u8>> {
        if job.print_token.is_empty() {
            return Err(Error::Export("preview job carries no print token".into()));
        }
        let mut browser = Browser::launch(self.options.clone())
            .await
            .map_err(|e| Error::Export(format!("browser launch failed: {e}")))?;
        let result = self
            .capture_one(&browser, job, slide_no, clicks)
            .await;
        let _ = browser.close().await;
        result
    }

    async fn capture_one(
        &self,
        browser: &Browser,
        job: &ExportJob,
        slide_no: u32,
        clicks: u32,
    ) -> Result<Vec<u8>> {
        let tab = browser
            .new_tab_at(&self.print_url(&job.deck_id, &job.print_token, "single", Some("all")))
            .await
            .map_err(|e| Error::Export(format!("tab open failed: {e}")))?;
        let scale = f64::from(job.dpi) / 96.0;
        let metrics = SetDeviceMetricsOverrideParams::builder()
            .width(i64::from(job.canvas_width))
            .height(i64::from(job.canvas_height))
            .device_scale_factor(scale)
            .mobile(false)
            .build()
            .map_err(|e: String| Error::Export(format!("device metrics params: {e}")))?;
        tab.page()
            .execute(metrics)
            .await
            .map_err(|e| Error::Export(format!("device metrics: {e}")))?;
        tab.evaluate(&format!("window.__deckGoto({slide_no}, {clicks})"))
            .await
            .map_err(|e| Error::Export(format!("goto slide {slide_no}: {e}")))?;
        wait_ready(&tab).await?;
        tab.screenshot_with(
            ScreenshotParams::builder()
                .format(CaptureScreenshotFormat::Png)
                .clip(
                    chromiumoxide::cdp::browser_protocol::page::Viewport::builder()
                        .x(0.0)
                        .y(0.0)
                        .width(f64::from(job.canvas_width))
                        .height(f64::from(job.canvas_height))
                        .scale(1.0)
                        .build()
                        .map_err(|e: String| Error::Export(format!("viewport params: {e}")))?,
                )
                .capture_beyond_viewport(true)
                .build(),
        )
        .await
        .map_err(|e| Error::Export(format!("screenshot slide {slide_no}: {e}")))
    }

    async fn run_job(
        &self,
        browser: &Browser,
        job: &ExportJob,
        canvas_w: u32,
        canvas_h: u32,
    ) -> Result<ExportOutput> {
        match job.format {
            ExportFormat::Pdf => self.export_pdf(browser, job, canvas_w, canvas_h).await,
            ExportFormat::Png => self.export_png(browser, job, canvas_w, canvas_h).await,
            ExportFormat::Pptx => {
                // M9: PNG frames feed the OOXML writer.
                let pngs = self.capture_png_pages(browser, job, canvas_w, canvas_h).await?;
                let bytes = crate::export::pptx::build_pptx(
                    &pngs,
                    canvas_w,
                    canvas_h,
                    &job.notes,
                )?;
                Ok(ExportOutput {
                    bytes,
                    mime: ExportFormat::Pptx.mime().to_string(),
                })
            }
        }
    }

    async fn export_pdf(
        &self,
        browser: &Browser,
        job: &ExportJob,
        canvas_w: u32,
        canvas_h: u32,
    ) -> Result<ExportOutput> {
        let tab = browser
            .new_tab_at(&self.print_url(&job.deck_id, &job.print_token, "pdf", None))
            .await
            .map_err(|e| Error::Export(format!("tab open failed: {e}")))?;
        wait_ready(&tab).await?;

        // paper size in inches (96 px/inch), @page already set by the page.
        let params = PrintToPdfParams::builder()
            .print_background(true)
            .prefer_css_page_size(true)
            .paper_width(f64::from(canvas_w) / 96.0)
            .paper_height(f64::from(canvas_h) / 96.0)
            .margin_top(0.0)
            .margin_bottom(0.0)
            .margin_left(0.0)
            .margin_right(0.0)
            .build();
        let bytes = tab
            .print_to_pdf(params)
            .await
            .map_err(|e| Error::Export(format!("printToPDF failed: {e}")))?;
        Ok(ExportOutput {
            bytes,
            mime: ExportFormat::Pdf.mime().to_string(),
        })
    }

    async fn export_png(
        &self,
        browser: &Browser,
        job: &ExportJob,
        canvas_w: u32,
        canvas_h: u32,
    ) -> Result<ExportOutput> {
        let pages = self.capture_png_pages(browser, job, canvas_w, canvas_h).await?;
        // One file per page, zipped: deck-000.png …
        let mut zip = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
        let options = zip::write::SimpleFileOptions::default();
        for (i, bytes) in pages.iter().enumerate() {
            zip.start_file(format!("deck-{:03}.png", i + 1), options)
                .map_err(|e| Error::Export(format!("zip: {e}")))?;
            std::io::Write::write_all(&mut zip, bytes).map_err(|e| Error::Export(format!("zip: {e}")))?;
        }
        let cursor = zip
            .finish()
            .map_err(|e| Error::Export(format!("zip: {e}")))?;
        Ok(ExportOutput {
            bytes: cursor.into_inner(),
            mime: "application/zip".to_string(),
        })
    }

    /// Drive `__deckGoto` across the requested pages, capturing each at
    /// the requested DPI.
    async fn capture_png_pages(
        &self,
        browser: &Browser,
        job: &ExportJob,
        canvas_w: u32,
        canvas_h: u32,
    ) -> Result<Vec<Vec<u8>>> {
        let tab = browser
            .new_tab_at(&self.print_url(&job.deck_id, &job.print_token, "single", Some("all")))
            .await
            .map_err(|e| Error::Export(format!("tab open failed: {e}")))?;

        // DPI scaling: 300 DPI = 3.125x of 96.
        let scale = f64::from(job.dpi) / 96.0;
        let metrics = SetDeviceMetricsOverrideParams::builder()
            .width(i64::from(canvas_w))
            .height(i64::from(canvas_h))
            .device_scale_factor(scale)
            .mobile(false)
            .build()
            .map_err(|e: String| Error::Export(format!("device metrics params: {e}")))?;
        tab.page()
            .execute(metrics)
            .await
            .map_err(|e| Error::Export(format!("device metrics: {e}")))?;

        let mut pages = Vec::new();
        for (idx, page_no) in job.pages.iter().enumerate() {
            tab.evaluate(&format!("window.__deckGoto({page_no}, 0)"))
                .await
                .map_err(|e| Error::Export(format!("goto page {page_no}: {e}")))?;
            // Per-slide settle: the wait protocol re-arms per render pass.
            wait_ready(&tab).await?;
            let bytes = tab
                .screenshot_with(
                    ScreenshotParams::builder()
                        .format(CaptureScreenshotFormat::Png)
                        .clip(
                            chromiumoxide::cdp::browser_protocol::page::Viewport::builder()
                                .x(0.0)
                                .y(0.0)
                                .width(f64::from(canvas_w))
                                .height(f64::from(canvas_h))
                                .scale(1.0)
                                .build()
                                .map_err(|e: String| Error::Export(format!("viewport params: {e}")))?,
                        )
                        .capture_beyond_viewport(true)
                        .build(),
                )
                .await
                .map_err(|e| Error::Export(format!("screenshot page {page_no}: {e}")))?;
            pages.push(bytes);
            let _ = idx;
        }
        Ok(pages)
    }
}
