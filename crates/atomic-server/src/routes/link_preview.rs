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
            status: job.status.to_string(),
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
        Some(job) => match (job.status, job.screenshot_jpeg_bytes) {
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
