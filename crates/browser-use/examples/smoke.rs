use browser_use::{Browser, BrowserOptions};

#[tokio::main]
async fn main() -> browser_use::Result<()> {
    let mut opts = BrowserOptions::default();
    opts.no_sandbox = true;
    let mut browser = Browser::launch(opts).await?;
    println!("version: {}", browser.version().await?);
    let tab = browser.new_tab_at("example.com").await?;
    println!("url: {:?}", tab.url().await?);
    println!("title: {:?}", tab.title().await?);
    tab.click("a").await?;
    println!("after click: {:?}", tab.url().await?);
    println!("text head: {:.60}", tab.text().await?.replace('\n', " "));
    tab.save_screenshot("/tmp/bu-smoke/iana.png").await?;
    browser.close().await?;
    Ok(())
}
