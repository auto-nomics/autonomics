#![cfg(feature = "e2e")]
//! Exercise the actual Chromium exporter against an intentionally slow print page.
use std::{io::{Read, Write}, net::TcpListener, sync::{Arc, atomic::{AtomicBool, Ordering}}, time::Duration};
use ppt_base::export::{DeckExporter, ExportJob, chromium::ChromiumExporter};
use ppt_base::store::ExportFormat;
use serde_json::json;

struct TestServer { origin: String, stopped: Arc<AtomicBool>, thread: Option<std::thread::JoinHandle<()>> }
impl TestServer {
    fn start() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let origin = format!("http://{}", listener.local_addr().unwrap());
        listener.set_nonblocking(true).unwrap();
        let stopped = Arc::new(AtomicBool::new(false));
        let flag = stopped.clone();
        let thread = std::thread::spawn(move || {
            while !flag.load(Ordering::Relaxed) {
                match listener.accept() {
                    Ok((mut stream, _)) => {
                        let mut buf = [0; 4096];
                        let read = stream.read(&mut buf).unwrap_or(0);
                        let request = String::from_utf8_lossy(&buf[..read]);
                        let body = if request.contains("deck=error") {
                            "<script>document.documentElement.dataset.deckError='mock payload failed'</script>"
                        } else if request.contains("deck=diagnose") {
                            r#"<!doctype html><style>html,body{margin:0;width:100%;height:100%;background:#eee}</style>
<div class="deck-print-stage"><div class="deck-slide" id="s1">first</div></div>
<script>
setTimeout(() => {
  window.__deckState = {slide:1,click:0};
  window.__deckGoto = (slide,click) => {
    document.documentElement.dataset.deckReady = 'false';
    setTimeout(() => {
      document.getElementById('s1').textContent = 'slide ' + slide;
      window.__deckState = {slide,click};
      window.__deckDiagnose = () => [
        { rule: 'text-overflow-canvas', severity: 'error',
          message: 'slide ' + slide + ' overflow', details: { overflow: 'right 24px' } }
      ];
      document.documentElement.dataset.deckReady = 'true';
    }, 300);
  };
  document.documentElement.dataset.deckReady = 'true';
}, 100);
</script>"#
                        } else {
                            r#"<!doctype html><style>html,body{margin:0;width:100%;height:100%;background:red}</style>
<script>
setTimeout(() => {
  window.__deckState = {slide:1,click:0};
  window.__deckGoto = (slide,click) => {
    if (slide === window.__deckState.slide && click === window.__deckState.click) return;
    document.documentElement.dataset.deckReady = 'false';
    setTimeout(() => {
      document.body.style.background = slide === 1 ? 'red' : 'blue';
      window.__deckState = {slide,click};
      document.documentElement.dataset.deckReady = 'true';
    }, 750);
  };
  document.documentElement.dataset.deckReady = 'true';
}, 900);
</script>"#
                        };
                        let response = format!("HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}", body.len(), body);
                        let _ = stream.write_all(response.as_bytes());
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => std::thread::sleep(Duration::from_millis(10)),
                    Err(_) => break,
                }
            }
        });
        Self { origin, stopped, thread: Some(thread) }
    }
}
impl Drop for TestServer {
    fn drop(&mut self) { self.stopped.store(true, Ordering::Relaxed); if let Some(thread) = self.thread.take() { let _ = thread.join(); } }
}

fn job(deck: &str) -> ExportJob {
    ExportJob { deck_id: deck.into(), export_id: "test".into(), format: ExportFormat::Png, dpi: 96, pages: vec![1, 2], canvas_width: 200, canvas_height: 120, notes: vec![], print_token: "test".into() }
}

#[tokio::test]
async fn slow_initialization_and_navigation_wait_for_the_correct_page() {
    let server = TestServer::start();
    let exporter = ChromiumExporter { origin: server.origin.clone(), options: browser_use::BrowserOptions { no_sandbox: true, ..Default::default() } };
    let output = exporter.export(job("test")).await.unwrap();
    let mut archive = zip::ZipArchive::new(std::io::Cursor::new(output.bytes)).unwrap();
    let mut first = Vec::new(); archive.by_index(0).unwrap().read_to_end(&mut first).unwrap();
    let mut second = Vec::new(); archive.by_index(1).unwrap().read_to_end(&mut second).unwrap();
    assert_ne!(first, second, "two distinct slide backgrounds must produce distinct PNGs");
    let preview = exporter.preview_png(&job("test"), 2, 1).await.unwrap();
    assert_eq!(preview, second, "preview must capture the committed target slide");
    let start = std::time::Instant::now();
    let error = exporter.preview_png(&job("error"), 2, 0).await.unwrap_err().to_string();
    assert!(error.contains("mock payload failed"), "{error}");
    assert!(start.elapsed() < Duration::from_secs(15), "initialization errors should not wait for the full timeout");
}

#[tokio::test]
async fn diagnose_slides_collects_findings_per_committed_page() {
    let server = TestServer::start();
    let exporter = ChromiumExporter { origin: server.origin.clone(), options: browser_use::BrowserOptions { no_sandbox: true, ..Default::default() } };
    let value = exporter.diagnose_slides(&job("diagnose"), &[(1, 0), (2, 1)]).await.unwrap();
    assert_eq!(value["1"][0]["message"], json!("slide 1 overflow"), "{value}");
    assert_eq!(value["2"][0]["message"], json!("slide 2 overflow"), "{value}");
    assert_eq!(value["2"][0]["rule"], json!("text-overflow-canvas"));
    assert_eq!(value["2"][0]["severity"], json!("error"));
}
