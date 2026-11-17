//! RSS subscription workflows: parsing, store CRUD/versioning, conditional
//! fetch against a local HTTP fixture, and item import article mapping.
use bib_base::BibShared;
use bib_base::rss::{self, FeedSpec, RssStore};
use tokio::io::{AsyncReadExt as _, AsyncWriteExt};

fn spec(url: &str) -> FeedSpec {
    FeedSpec {
        name: "Fixture feed".into(),
        url: url.into(),
        ..Default::default()
    }
}

#[test]
fn parses_rss2_and_atom_items() {
    let rss2 = r#"
    <rss version="2.0" xmlns:dc="http://purl.org/dc/elements/1.1/">
      <channel><title>Journal</title>
        <item>
          <title>Paper &amp; One</title>
          <link>https://example.org/paper/10.1000/one</link>
          <guid isPermaLink="false">guid-one</guid>
          <dc:creator>Family, Given</dc:creator>
          <pubDate>Fri, 02 Jan 2026 00:00:00 GMT</pubDate>
          <description>&lt;b&gt;Bold&lt;/b&gt; summary text</description>
        </item>
      </channel>
    </rss>"#;
    let items = rss::parse_feed(rss2).unwrap();
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].guid, "guid-one");
    assert_eq!(items[0].title, "Paper & One");
    assert_eq!(items[0].authors, ["Family, Given"]);
    assert_eq!(items[0].summary.as_deref(), Some("Bold summary text"));
    assert!(items[0].published_ms.unwrap() > 1_700_000_000_000);

    let atom = r#"
    <feed xmlns="http://www.w3.org/2005/Atom">
      <entry>
        <title>Arxiv entry</title>
        <id>urn:uuid:4c1f</id>
        <link href="https://arxiv.org/abs/2601.01234"/>
        <published>2026-02-03T04:05:06Z</published>
        <summary>An atom summary</summary>
        <author><name>Ada Lovelace</name></author>
      </entry>
    </feed>"#;
    let items = rss::parse_feed(atom).unwrap();
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].guid, "urn:uuid:4c1f");
    assert_eq!(
        items[0].link.as_deref(),
        Some("https://arxiv.org/abs/2601.01234")
    );
    assert_eq!(items[0].authors, ["Ada Lovelace"]);
}

#[tokio::test]
async fn feed_crud_uses_optimistic_versions() {
    let store = RssStore::open(":memory:").await.unwrap();
    let feed = store
        .create_feed(spec("https://example.org/feed.xml"))
        .await
        .unwrap();
    assert_eq!(feed.version, 0);
    assert_eq!(feed.item_count, 0);

    // Stale versions conflict; concurrent URL duplicates are rejected.
    let err = store
        .replace_feed(&feed.id, 7, spec("https://example.org/other.xml"))
        .await
        .unwrap_err();
    assert!(matches!(err, rss::RssError::Conflict(_)));
    let err = store
        .create_feed(spec("https://example.org/feed.xml"))
        .await
        .unwrap_err();
    assert!(matches!(err, rss::RssError::Conflict(_)));

    let updated = store
        .replace_feed(
            &feed.id,
            0,
            FeedSpec {
                name: "Renamed".into(),
                ..spec("https://example.org/feed.xml")
            },
        )
        .await
        .unwrap();
    assert_eq!(updated.version, 1);
    assert_eq!(updated.spec.name, "Renamed");

    let paused = store.set_enabled(&feed.id, 1, false).await.unwrap();
    assert!(!paused.spec.enabled);
    // Paused feeds are not due.
    assert!(store.due_feeds(i64::MAX, 10).await.unwrap().is_empty());
    store.delete_feed(&feed.id, 2).await.unwrap();
    assert!(store.list_feeds().await.unwrap().is_empty());
}

/// Minimal one-shot HTTP server: answers with fixed headers and a body.
async fn serve(status: &'static str, headers: &'static str, body: &'static str) -> String {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let origin = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        let (mut read, mut write) = stream.into_split();
        // Drain the request head (we ignore its content).
        let mut buf = [0u8; 4096];
        let mut seen = Vec::new();
        while !seen.windows(4).any(|w| w == b"\r\n\r\n") {
            let n = read.read(&mut buf).await.unwrap();
            if n == 0 {
                break;
            }
            seen.extend_from_slice(&buf[..n]);
        }
        let response = format!(
            "HTTP/1.1 {status}\r\n{headers}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        );
        write.write_all(response.as_bytes()).await.unwrap();
    });
    origin
}

#[tokio::test]
async fn check_feed_dedups_and_supports_not_modified() {
    let body = r#"<rss version="2.0"><channel>
      <item><title>First</title><guid>g1</guid><link>https://example.org/doi/10.1000/first</link></item>
      <item><title>Second</title><guid>g2</guid><link>https://arxiv.org/abs/2601.99999</link></item>
    </channel></rss>"#;
    let origin = serve("200 OK", "ETag: \"v1\"", body).await;
    let shared = BibShared::open_in_memory().await.unwrap();
    let feed = shared
        .rss
        .create_feed(spec(&format!("{origin}/feed")))
        .await
        .unwrap();

    let outcome = rss::check_feed(&shared, &feed.id).await.unwrap();
    assert_eq!(outcome.status, "fetched");
    assert_eq!(outcome.new_items, 2);
    let feed = shared.rss.get_feed(&feed.id).await.unwrap();
    assert_eq!(feed.etag.as_deref(), Some("\"v1\""));

    // Re-running the same body adds nothing.
    let origin2 = serve("200 OK", "", body).await;
    shared
        .rss
        .replace_feed(&feed.id, feed.version, spec(&format!("{origin2}/feed")))
        .await
        .unwrap();
    let feed = shared.rss.get_feed(&feed.id).await.unwrap();
    let outcome = rss::check_feed(&shared, &feed.id).await.unwrap();
    assert_eq!(outcome.status, "fetched");
    assert_eq!(outcome.new_items, 0);

    // 304 keeps the inbox untouched.
    let origin3 = serve("304 Not Modified", "", "").await;
    shared
        .rss
        .replace_feed(&feed.id, feed.version, spec(&format!("{origin3}/feed")))
        .await
        .unwrap();
    let feed = shared.rss.get_feed(&feed.id).await.unwrap();
    let outcome = rss::check_feed(&shared, &feed.id).await.unwrap();
    assert_eq!(outcome.status, "not_modified");

    let (items, next) = shared.rss.items(None, None, 50).await.unwrap();
    assert!(next.is_none());
    assert_eq!(items.len(), 2);

    // Import maps identifiers: DOI from the link, arXiv from the abs URL.
    let first = items.iter().find(|i| i.guid == "g1").unwrap();
    let article = rss::article_from_item(first);
    assert!(
        article
            .identifiers
            .iter()
            .any(|id| { id.kind == bib_types::IdKind::Doi && id.value == "10.1000/first" })
    );
    let second = items.iter().find(|i| i.guid == "g2").unwrap();
    let article = rss::article_from_item(second);
    assert!(
        article
            .identifiers
            .iter()
            .any(|id| { id.kind == bib_types::IdKind::Arxiv && id.value == "2601.99999" })
    );
    shared.bib.upsert_article(&article).await.unwrap();
    shared
        .rss
        .mark_imported(&second.id, &article.id)
        .await
        .unwrap();
    let (after, _) = shared.rss.items(None, None, 50).await.unwrap();
    assert!(
        after
            .iter()
            .find(|i| i.id == second.id)
            .unwrap()
            .imported_article_id
            .is_some()
    );

    // A failing origin is recorded on the feed without losing items.
    let origin4 = serve("500 Internal Server Error", "", "").await;
    shared
        .rss
        .replace_feed(
            &feed.id,
            shared.rss.get_feed(&feed.id).await.unwrap().version,
            spec(&format!("{origin4}/feed")),
        )
        .await
        .unwrap();
    assert!(rss::check_feed(&shared, &feed.id).await.is_err());
    let feed = shared.rss.get_feed(&feed.id).await.unwrap();
    assert!(feed.last_error.is_some());
    let (kept, _) = shared.rss.items(None, None, 50).await.unwrap();
    assert_eq!(kept.len(), 2);
}

#[tokio::test]
async fn scheduler_keeps_http_errors_on_the_feed_and_manual_success_clears_them() {
    let shared = BibShared::open_in_memory().await.unwrap();
    let origin = serve("404 Not Found", "", "").await;
    let feed = shared
        .rss
        .create_feed(spec(&format!("{origin}/missing")))
        .await
        .unwrap();
    let scheduler = rss::RssScheduler::start(shared.clone(), 1);
    tokio::time::timeout(std::time::Duration::from_secs(10), async {
        loop {
            let current = shared.rss.get_feed(&feed.id).await.unwrap();
            if current.last_error.is_some() && scheduler.status().active_checks == 0 {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    assert!(
        shared
            .rss
            .get_feed(&feed.id)
            .await
            .unwrap()
            .last_error
            .unwrap()
            .contains("404")
    );
    assert!(scheduler.status().running);
    assert!(scheduler.status().last_error.is_none());

    let origin = serve(
        "200 OK",
        "",
        "<rss version=\"2.0\"><channel><title>Recovered</title></channel></rss>",
    )
    .await;
    shared
        .rss
        .replace_feed(&feed.id, feed.version, spec(&format!("{origin}/feed")))
        .await
        .unwrap();
    rss::check_feed(&shared, &feed.id).await.unwrap();
    assert!(
        shared
            .rss
            .get_feed(&feed.id)
            .await
            .unwrap()
            .last_error
            .is_none()
    );
    assert!(scheduler.status().last_error.is_none());
    scheduler.shutdown().await;
}
