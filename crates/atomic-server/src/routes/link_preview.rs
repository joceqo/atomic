//! Link preview route (server-side fetch only)

use crate::link_preview_queue::ScreenshotJobStatus;
use crate::state::AppState;
use actix_web::{web, HttpResponse};
use scraper::{Html, Selector};
use serde::{Deserialize, Serialize};
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
use url::Url;

#[derive(Deserialize)]
pub struct LinkPreviewQuery {
    pub url: String,
}

#[derive(Deserialize)]
pub struct LinkPreviewProxyImageQuery {
    /// Absolute image URL (typically og:image).
    pub url: String,
    /// Page URL the preview is for; sent as Referer when fetching the image (hotlink protection).
    pub referer: String,
}

#[derive(Serialize)]
pub struct LinkPreviewResponse {
    pub url: String,
    pub title: Option<String>,
    pub description: Option<String>,
    pub image: Option<String>,
    pub site_name: Option<String>,
}

#[derive(Deserialize)]
pub struct LinkScreenshotEnqueueRequest {
    pub url: String,
}

#[derive(Serialize)]
pub struct LinkScreenshotEnqueueResponse {
    pub job_id: String,
    pub status: String,
}

#[derive(Serialize)]
pub struct LinkScreenshotStatusResponse {
    pub job_id: String,
    pub status: String,
    pub error: Option<String>,
}

pub async fn get_link_preview(query: web::Query<LinkPreviewQuery>) -> HttpResponse {
    match fetch_link_preview(&query.url).await {
        Ok(preview) => HttpResponse::Ok().json(preview),
        Err(error) => HttpResponse::BadGateway().json(serde_json::json!({ "error": error })),
    }
}

pub async fn get_link_preview_proxy_image(query: web::Query<LinkPreviewProxyImageQuery>) -> HttpResponse {
    let image_url = match Url::parse(&query.url) {
        Ok(u) => u,
        Err(_) => {
            return HttpResponse::BadRequest().json(serde_json::json!({ "error": "Invalid image URL" }));
        }
    };
    let referer_url = match Url::parse(&query.referer) {
        Ok(u) => u,
        Err(_) => {
            return HttpResponse::BadRequest().json(serde_json::json!({ "error": "Invalid referer URL" }));
        }
    };

    if !matches!(image_url.scheme(), "http" | "https")
        || !matches!(referer_url.scheme(), "http" | "https")
    {
        return HttpResponse::BadRequest().json(serde_json::json!({ "error": "Only http/https URLs are allowed" }));
    }

    if let Some(h) = image_url.host_str() {
        if is_forbidden_host(h) {
            return HttpResponse::BadRequest().json(serde_json::json!({ "error": "Image host is not allowed" }));
        }
    }
    if let Some(h) = referer_url.host_str() {
        if is_forbidden_host(h) {
            return HttpResponse::BadRequest().json(serde_json::json!({ "error": "Referer host is not allowed" }));
        }
    }

    if !preview_image_host_allowed(&referer_url, &image_url) {
        return HttpResponse::BadRequest().json(serde_json::json!({
            "error": "Image host does not match preview page host"
        }));
    }

    let client = match reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(15))
        .redirect(reqwest::redirect::Policy::limited(5))
        .build()
    {
        Ok(c) => c,
        Err(e) => {
            tracing::warn!(error = %e, "failed to build proxy image HTTP client");
            return HttpResponse::InternalServerError().finish();
        }
    };

    let response = match client
        .get(image_url.clone())
        .header(
            reqwest::header::USER_AGENT,
            "AtomicLinkPreview/1.0 (+https://github.com/kenforthewin/atomic)",
        )
        .header(reqwest::header::REFERER, referer_url.as_str())
        .send()
        .await
    {
        Ok(r) => r,
        Err(e) => {
            tracing::debug!(error = %e, "proxy image fetch failed");
            return HttpResponse::BadGateway().json(serde_json::json!({ "error": "Failed to fetch image" }));
        }
    };

    if !response.status().is_success() {
        return HttpResponse::BadGateway().json(serde_json::json!({
            "error": format!("Image URL returned {}", response.status())
        }));
    }

    let content_type = response
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("application/octet-stream")
        .split(';')
        .next()
        .unwrap_or("application/octet-stream")
        .trim()
        .to_string();

    let bytes = match response.bytes().await {
        Ok(b) => b,
        Err(_) => {
            return HttpResponse::BadGateway().json(serde_json::json!({ "error": "Failed to read image body" }));
        }
    };

    // Basic size cap (avoid huge payloads in memory)
    const MAX_BYTES: usize = 6 * 1024 * 1024;
    if bytes.len() > MAX_BYTES {
        return HttpResponse::BadRequest().json(serde_json::json!({ "error": "Image too large" }));
    }

    HttpResponse::Ok()
        .content_type(content_type)
        .body(bytes.to_vec())
}

pub async fn enqueue_link_preview_screenshot(
    state: web::Data<AppState>,
    body: web::Json<LinkScreenshotEnqueueRequest>,
) -> HttpResponse {
    let parsed = match Url::parse(&body.url) {
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

    let job_id = match state.link_preview_queue.enqueue(parsed.to_string()).await {
        Ok(id) => id,
        Err(error) => {
            return HttpResponse::ServiceUnavailable().json(serde_json::json!({
                "error": error
            }))
        }
    };
    HttpResponse::Ok().json(LinkScreenshotEnqueueResponse {
        job_id,
        status: "queued".to_string(),
    })
}

pub async fn get_link_preview_screenshot_status(
    state: web::Data<AppState>,
    path: web::Path<String>,
) -> HttpResponse {
    let job_id = path.into_inner();
    match state.link_preview_queue.get(&job_id).await {
        Some(job) => HttpResponse::Ok().json(LinkScreenshotStatusResponse {
            job_id: job.id,
            status: format!("{:?}", job.status).to_ascii_lowercase(),
            error: job.error,
        }),
        None => HttpResponse::NotFound().json(serde_json::json!({
            "error": "Job not found"
        })),
    }
}

pub async fn get_link_preview_screenshot_image(
    state: web::Data<AppState>,
    path: web::Path<String>,
) -> HttpResponse {
    let job_id = path.into_inner();
    match state.link_preview_queue.get(&job_id).await {
        Some(job) => match (job.status, job.image_bytes) {
            (ScreenshotJobStatus::Completed, Some(bytes)) => {
                HttpResponse::Ok().content_type("image/jpeg").body(bytes)
            }
            (ScreenshotJobStatus::Failed, _) => HttpResponse::BadGateway()
                .json(serde_json::json!({"error": job.error.unwrap_or_else(|| "Screenshot generation failed".to_string())})),
            _ => HttpResponse::Accepted().json(serde_json::json!({
                "status": "processing"
            })),
        },
        None => HttpResponse::NotFound().json(serde_json::json!({
            "error": "Job not found"
        })),
    }
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

/// Allow proxying only when image and page look like the same site (same host or same last-two DNS labels).
fn preview_image_host_allowed(referer: &Url, image: &Url) -> bool {
    let Some(rh) = referer.host_str() else {
        return false;
    };
    let Some(ih) = image.host_str() else {
        return false;
    };
    let r = rh.trim_end_matches('.').to_ascii_lowercase();
    let i = ih.trim_end_matches('.').to_ascii_lowercase();
    if r == i {
        return true;
    }
    let r_labels: Vec<&str> = r.split('.').collect();
    let i_labels: Vec<&str> = i.split('.').collect();
    if r_labels.len() >= 2 && i_labels.len() >= 2 {
        let r_root = format!("{}.{}", r_labels[r_labels.len() - 2], r_labels[r_labels.len() - 1]);
        let i_root = format!("{}.{}", i_labels[i_labels.len() - 2], i_labels[i_labels.len() - 1]);
        if r_root == i_root {
            return true;
        }
    }
    false
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
    let octets = ip.octets();
    let is_doc_range = (octets[0] == 192 && octets[1] == 0 && octets[2] == 2)
        || (octets[0] == 198 && octets[1] == 51 && octets[2] == 100)
        || (octets[0] == 203 && octets[1] == 0 && octets[2] == 113);
    ip.is_private()
        || ip.is_loopback()
        || ip.is_link_local()
        || ip.is_broadcast()
        || is_doc_range
        || ip.is_unspecified()
        || ip.octets()[0] == 0
}

fn is_private_ipv6(ip: Ipv6Addr) -> bool {
    let segments = ip.segments();
    // 2001:db8::/32 documentation range
    let is_doc_range = segments[0] == 0x2001 && segments[1] == 0x0db8;
    ip.is_loopback()
        || ip.is_unspecified()
        || ip.is_unique_local()
        || ip.is_unicast_link_local()
        || is_doc_range
}

pub async fn fetch_link_preview(url: &str) -> Result<LinkPreviewResponse, String> {
    let parsed = Url::parse(url).map_err(|_| "Invalid URL".to_string())?;
    if !matches!(parsed.scheme(), "http" | "https") {
        return Err("Only http/https URLs are allowed".to_string());
    }
    if let Some(host) = parsed.host_str() {
        if is_forbidden_host(host) {
            return Err("URL host is not allowed".to_string());
        }
    }

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(8))
        .redirect(reqwest::redirect::Policy::limited(5))
        .build()
        .map_err(|e| format!("failed to build HTTP client: {e}"))?;

    let response = client
        .get(parsed.clone())
        .header(
            reqwest::header::USER_AGENT,
            "AtomicLinkPreview/1.0 (+https://github.com/kenforthewin/atomic)",
        )
        .send()
        .await
        .map_err(|e| format!("failed to fetch URL: {e}"))?;

    if !response.status().is_success() {
        return Err(format!("Remote server returned {}", response.status()));
    }

    let body = response
        .text()
        .await
        .map_err(|_| "Failed to read URL response body".to_string())?;

    let document = Html::parse_document(&body);
    let title = first_meta_content(&document, "property", "og:title")
        .or_else(|| first_text(&document, "title"));
    let description = first_meta_content(&document, "property", "og:description")
        .or_else(|| first_meta_content(&document, "name", "description"));
    let image = first_meta_content(&document, "property", "og:image");
    let site_name = first_meta_content(&document, "property", "og:site_name");

    Ok(LinkPreviewResponse {
        url: parsed.to_string(),
        title,
        description,
        image,
        site_name,
    })
}
