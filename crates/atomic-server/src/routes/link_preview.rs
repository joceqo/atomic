//! Link preview route (server-side fetch only)

use actix_web::{web, HttpResponse};
use scraper::{Html, Selector};
use serde::{Deserialize, Serialize};
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
use url::Url;

#[derive(Deserialize)]
pub struct LinkPreviewQuery {
    pub url: String,
}

#[derive(Serialize)]
pub struct LinkPreviewResponse {
    pub url: String,
    pub title: Option<String>,
    pub description: Option<String>,
    pub image: Option<String>,
    pub site_name: Option<String>,
}

pub async fn get_link_preview(query: web::Query<LinkPreviewQuery>) -> HttpResponse {
    let parsed = match Url::parse(&query.url) {
        Ok(url) => url,
        Err(_) => {
            return HttpResponse::BadRequest().json(serde_json::json!({
                "error": "Invalid URL"
            }))
        }
    };

    if !matches!(parsed.scheme(), "http" | "https") {
        return HttpResponse::BadRequest().json(serde_json::json!({
            "error": "Only http/https URLs are allowed"
        }));
    }

    if let Some(host) = parsed.host_str() {
        if is_forbidden_host(host) {
            return HttpResponse::BadRequest().json(serde_json::json!({
                "error": "URL host is not allowed"
            }));
        }
    }

    let client = match reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(8))
        .redirect(reqwest::redirect::Policy::limited(5))
        .build()
    {
        Ok(c) => c,
        Err(e) => {
            tracing::warn!(error = %e, "failed to build link preview HTTP client");
            return HttpResponse::InternalServerError().finish();
        }
    };

    let response = match client
        .get(parsed.clone())
        .header(
            reqwest::header::USER_AGENT,
            "AtomicLinkPreview/1.0 (+https://github.com/kenforthewin/atomic)",
        )
        .send()
        .await
    {
        Ok(r) => r,
        Err(e) => {
            tracing::debug!(error = %e, url = %parsed, "failed to fetch preview URL");
            return HttpResponse::BadGateway().json(serde_json::json!({
                "error": "Failed to fetch URL"
            }));
        }
    };

    if !response.status().is_success() {
        return HttpResponse::BadGateway().json(serde_json::json!({
            "error": format!("Remote server returned {}", response.status())
        }));
    }

    let body = match response.text().await {
        Ok(t) => t,
        Err(_) => {
            return HttpResponse::BadGateway().json(serde_json::json!({
                "error": "Failed to read URL response body"
            }));
        }
    };

    let document = Html::parse_document(&body);
    let title = first_meta_content(&document, "property", "og:title")
        .or_else(|| first_text(&document, "title"));
    let description = first_meta_content(&document, "property", "og:description")
        .or_else(|| first_meta_content(&document, "name", "description"));
    let image = first_meta_content(&document, "property", "og:image");
    let site_name = first_meta_content(&document, "property", "og:site_name");

    HttpResponse::Ok().json(LinkPreviewResponse {
        url: parsed.to_string(),
        title,
        description,
        image,
        site_name,
    })
}

fn first_meta_content(document: &Html, attr_name: &str, attr_value: &str) -> Option<String> {
    let selector = Selector::parse(&format!("meta[{attr_name}=\"{attr_value}\"]")).ok()?;
    document
        .select(&selector)
        .find_map(|el| el.value().attr("content").map(|s| s.trim().to_string()))
        .filter(|s| !s.is_empty())
}

fn first_text(document: &Html, selector: &str) -> Option<String> {
    let selector = Selector::parse(selector).ok()?;
    document
        .select(&selector)
        .next()
        .map(|el| el.text().collect::<String>().trim().to_string())
        .filter(|s| !s.is_empty())
}

fn is_forbidden_host(host: &str) -> bool {
    let normalized = host.trim().trim_end_matches('.').to_ascii_lowercase();
    if normalized == "localhost" || normalized == "0.0.0.0" {
        return true;
    }

    if let Ok(ip) = normalized.parse::<IpAddr>() {
        return is_private_ip(ip);
    }

    false
}

fn is_private_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => is_private_ipv4(v4),
        IpAddr::V6(v6) => is_private_ipv6(v6),
    }
}

fn is_private_ipv4(ip: Ipv4Addr) -> bool {
    ip.is_private()
        || ip.is_loopback()
        || ip.is_link_local()
        || ip.is_broadcast()
        || ip.is_documentation()
        || ip.is_unspecified()
        || ip.octets()[0] == 0
}

fn is_private_ipv6(ip: Ipv6Addr) -> bool {
    ip.is_loopback()
        || ip.is_unspecified()
        || ip.is_unique_local()
        || ip.is_unicast_link_local()
        || ip.is_documentation()
}
