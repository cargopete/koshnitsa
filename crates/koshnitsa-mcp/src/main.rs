mod credentials;
mod server;

use std::io::{IsTerminal, Read};

use anyhow::{Context, Result, bail};
use clap::{Parser, Subcommand};
use koshnitsa_client::{Client, Config, Cookie};
use rmcp::ServiceExt;

/// Unofficial MCP server for the ebag.bg grocery shop. Experimental; use at your own risk.
#[derive(Parser)]
#[command(version)]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand)]
enum Command {
    /// Run the MCP server on stdio (the default).
    Serve,
    /// Store the Cookie header from a logged-in ebag.bg tab in the OS keychain.
    Login {
        /// The Cookie header value. Read from stdin when omitted, which keeps it out of shell history.
        #[arg(long, conflicts_with = "browser")]
        cookie: Option<String>,
        /// Read the session straight from a browser you are logged in with instead.
        #[arg(long, value_enum)]
        browser: Option<Browser>,
    },
    /// Remove the stored cookie.
    Logout,
    /// Check whether the stored session is still logged in.
    Status,
}

#[derive(Clone, Copy, clap::ValueEnum)]
enum Browser {
    Chrome,
    Brave,
    Arc,
    Edge,
    Firefox,
    Safari,
}

impl Browser {
    fn cookie_header(self) -> Result<String> {
        let domains = Some(vec!["ebag.bg".to_string()]);
        let cookies = match self {
            Browser::Chrome => rookie::chrome(domains),
            Browser::Brave => rookie::brave(domains),
            Browser::Arc => rookie::arc(domains),
            Browser::Edge => rookie::edge(domains),
            Browser::Firefox => rookie::firefox(domains),
            Browser::Safari => rookie::safari(domains),
        }
        .map_err(|e| anyhow::anyhow!("reading the browser's cookies: {e}"))?;
        // `domains` is a substring filter, so keep only ebag.bg and its subdomains.
        let header = cookies
            .iter()
            .filter(|c| {
                let d = c.domain.trim_start_matches('.');
                d == "ebag.bg" || d.ends_with(".ebag.bg")
            })
            .map(|c| format!("{}={}", c.name, c.value))
            .collect::<Vec<_>>()
            .join("; ");
        if header.is_empty() {
            bail!("no ebag.bg cookies found; log in at https://ebag.bg in that browser first");
        }
        Ok(header)
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    // stdout belongs to the MCP transport.
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "koshnitsa=info,koshnitsa_client=info".into()),
        )
        .init();

    match Cli::parse().command.unwrap_or(Command::Serve) {
        Command::Serve => serve().await,
        Command::Login { cookie, browser } => login(cookie, browser).await,
        Command::Logout => {
            let removed = credentials::delete()?;
            eprintln!(
                "{}",
                if removed {
                    "Cookie removed."
                } else {
                    "No cookie was stored."
                }
            );
            Ok(())
        }
        Command::Status => status().await,
    }
}

async fn serve() -> Result<()> {
    let cookie = credentials::load()?;
    if cookie.is_none() {
        tracing::warn!("no eBag cookie stored; only search, product and slot tools will work");
    }
    let client = Client::new(Config::default(), cookie)?;
    let service = server::Koshnitsa::new(client)
        .serve(rmcp::transport::stdio())
        .await
        .context("starting the MCP server")?;
    service.waiting().await?;
    Ok(())
}

async fn login(cookie: Option<String>, browser: Option<Browser>) -> Result<()> {
    let raw = match (cookie, browser) {
        (Some(c), _) => c,
        (None, Some(b)) => b.cookie_header()?,
        (None, None) => {
            if std::io::stdin().is_terminal() {
                eprintln!(
                    "1. Log in at https://ebag.bg in your browser.\n\
                     2. DevTools > Network, click any request to ebag.bg, copy the Cookie request header.\n\
                     3. Paste it here and press Ctrl-D."
                );
            }
            let mut buf = String::new();
            std::io::stdin().read_to_string(&mut buf)?;
            buf
        }
    };
    let cookie = Cookie::parse(&raw)?;
    let client = Client::new(Config::default(), Some(cookie.clone()))?;
    let user = client
        .user()
        .await
        .context("eBag did not accept that cookie")?;
    credentials::store(&cookie)?;
    let name = if user.first_name.is_empty() {
        "you"
    } else {
        &user.first_name
    };
    eprintln!("Logged in as {name}. Cookie stored in the OS keychain.");
    Ok(())
}

async fn status() -> Result<()> {
    let Some(cookie) = credentials::load()? else {
        bail!("not logged in; run `koshnitsa login`");
    };
    let user = Client::new(Config::default(), Some(cookie))?.user().await?;
    eprintln!(
        "Session valid{}.",
        if user.first_name.is_empty() {
            String::new()
        } else {
            format!(" for {}", user.first_name)
        }
    );
    Ok(())
}
