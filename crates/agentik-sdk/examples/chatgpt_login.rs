//! ChatGPT 订阅 OAuth 真实登录脚本（原 `tests/chatgpt_oauth_e2e.rs` 的
//! mock 版已删除——此脚本打真环境，换取可用的 [`TokenBlob`]）。
//!
//! 走真实 `auth.openai.com`（Codex CLI 同款流程）：生成 PKCE → 起
//! `127.0.0.1:1455` 回调 → 浏览器授权 → 换 token → 校验 → 把完整
//! [`TokenBlob`] JSON 打到 **stdout**；人类可读日志全部走 stderr，因此
//! 可直接重定向落盘：
//!
//! ```text
//! cargo run -p agentik-sdk --example chatgpt_login > chatgpt_blob.json
//! ```
//!
//! 注意：
//! - 授权等待时限 10 分钟（`LOGIN_TIMEOUT`）；
//! - 回调地址是 `http://localhost:{1455|1457}/auth/callback`，浏览器必须
//!   在本机（或把 1455 端口转发到本机），否则授权码回不到脚本；
//! - 登录后用新 token 实拉一次远程模型目录做验证（只读）；验证失败不
//!   影响 blob 输出。

use std::process::Command;

use agentik_sdk::provider::openai::oauth::login_flow;
use agentik_sdk::provider::openai::{DEFAULT_BASE_URL, OpenaiProvider};

/// 尽力打开浏览器；失败仅提示手动复制 URL。
fn open_browser(url: &str) {
    let spawned = if cfg!(target_os = "macos") {
        Command::new("open").arg(url).spawn()
    } else if cfg!(target_os = "windows") {
        Command::new("cmd").args(["/C", "start", url]).spawn()
    } else {
        Command::new("xdg-open").arg(url).spawn()
    };
    if spawned.is_err() {
        eprintln!("（未能自动打开浏览器，请手动复制上方 URL 到浏览器）");
    }
}

#[tokio::main]
async fn main() {
    let (authorize_url, waiter) = match login_flow() {
        Ok(pair) => pair,
        Err(e) => {
            eprintln!("登录初始化失败：{e}");
            std::process::exit(1);
        }
    };

    eprintln!("ChatGPT 订阅 OAuth 登录（Codex CLI 流程，等待授权 10 分钟超时）");
    eprintln!();
    eprintln!("请在浏览器完成授权：");
    eprintln!("  {authorize_url}");
    eprintln!();
    eprintln!("（授权成功后浏览器会跳回本机 127.0.0.1:1455/1457）");
    open_browser(&authorize_url);

    let blob = match waiter.wait().await {
        Ok(blob) => blob,
        Err(e) => {
            eprintln!("登录失败：{e}");
            std::process::exit(1);
        }
    };

    let expires = blob
        .access_token_expires_in()
        .map(|d| format!("{:.1} 小时", d.as_secs_f64() / 3600.0))
        .unwrap_or_else(|| "未知（access token 无 exp claim）".to_string());
    eprintln!();
    eprintln!("登录成功：");
    eprintln!("  email        : {}", blob.email.as_deref().unwrap_or("（无）"));
    eprintln!("  订阅计划     : {}", blob.plan_type.as_deref().unwrap_or("（未知）"));
    eprintln!("  account id   : {}", blob.account_id);
    eprintln!("  access token 剩余有效期: {expires}");

    // 尽力验证：用新 token 实拉远程模型目录（只读 GET）。
    match OpenaiProvider::fetch_remote_catalog(
        DEFAULT_BASE_URL,
        &blob.access_token,
        &blob.account_id,
    )
    .await
    {
        Ok(catalogue) => {
            let names: Vec<&str> = catalogue.iter().map(|m| m.model_name.as_str()).collect();
            eprintln!(
                "  目录验证     : 通过，可用模型 {} 个: {}",
                names.len(),
                names.join(", ")
            );
        }
        Err(e) => {
            eprintln!("  目录验证     : 失败（{e}）；blob 照常输出，不影响本身可用性");
        }
    }

    // stdout 唯一输出：完整 TokenBlob JSON（即 providers.api_key 列的存储形态）。
    match blob.to_json() {
        Ok(json) => println!("{json}"),
        Err(e) => {
            eprintln!("token blob 序列化失败：{e}");
            std::process::exit(1);
        }
    }
}
