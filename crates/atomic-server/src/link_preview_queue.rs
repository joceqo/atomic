use std::collections::HashMap;
use std::sync::Arc;

use tokio::sync::{mpsc, RwLock};

use crate::routes::link_preview::fetch_link_preview;

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ScreenshotJobStatus {
    Pending,
    Processing,
    Completed,
    Failed,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ScreenshotJob {
    pub id: String,
    pub url: String,
    pub status: ScreenshotJobStatus,
    pub image_bytes: Option<Vec<u8>>,
    pub error: Option<String>,
}

#[derive(Clone)]
pub struct LinkPreviewQueue {
    jobs: Arc<RwLock<HashMap<String, ScreenshotJob>>>,
    tx: mpsc::Sender<String>,
}

impl LinkPreviewQueue {
    pub fn new(buffer: usize) -> (Self, mpsc::Receiver<String>) {
        let (tx, rx) = mpsc::channel(buffer);
        (
            Self {
                jobs: Arc::new(RwLock::new(HashMap::new())),
                tx,
            },
            rx,
        )
    }

    pub async fn enqueue(&self, url: String) -> Result<String, String> {
        let id = uuid::Uuid::new_v4().to_string();
        let job = ScreenshotJob {
            id: id.clone(),
            url,
            status: ScreenshotJobStatus::Pending,
            image_bytes: None,
            error: None,
        };
        self.jobs.write().await.insert(id.clone(), job);
        self.tx
            .send(id.clone())
            .await
            .map_err(|_| "queue unavailable".to_string())?;
        Ok(id)
    }

    pub async fn get(&self, id: &str) -> Option<ScreenshotJob> {
        self.jobs.read().await.get(id).cloned()
    }

    async fn set_processing(&self, id: &str) {
        if let Some(job) = self.jobs.write().await.get_mut(id) {
            job.status = ScreenshotJobStatus::Processing;
            job.error = None;
        }
    }

    async fn set_completed(&self, id: &str, image_bytes: Vec<u8>) {
        if let Some(job) = self.jobs.write().await.get_mut(id) {
            job.status = ScreenshotJobStatus::Completed;
            job.image_bytes = Some(image_bytes);
            job.error = None;
        }
    }

    async fn set_failed(&self, id: &str, error: String) {
        if let Some(job) = self.jobs.write().await.get_mut(id) {
            job.status = ScreenshotJobStatus::Failed;
            job.error = Some(error);
            job.image_bytes = None;
        }
    }
}

pub fn start_link_preview_worker(queue: LinkPreviewQueue, mut rx: mpsc::Receiver<String>) {
    tokio::spawn(async move {
        while let Some(job_id) = rx.recv().await {
            let Some(job) = queue.get(&job_id).await else {
                continue;
            };
            queue.set_processing(&job_id).await;
            match generate_preview_image(&job.url).await {
                Ok(bytes) => queue.set_completed(&job_id, bytes).await,
                Err(error) => queue.set_failed(&job_id, error).await,
            }
        }
    });
}

async fn generate_preview_image(url: &str) -> Result<Vec<u8>, String> {
    let preview = fetch_link_preview(url).await?;
    let image_url = preview
        .image
        .ok_or_else(|| "No preview image available for this URL".to_string())?;

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(12))
        .redirect(reqwest::redirect::Policy::limited(5))
        .build()
        .map_err(|e| format!("failed to build image client: {e}"))?;

    let response = client
        .get(&image_url)
        .header(
            reqwest::header::USER_AGENT,
            "AtomicLinkPreview/1.0 (+https://github.com/kenforthewin/atomic)",
        )
        .send()
        .await
        .map_err(|e| format!("failed to fetch preview image: {e}"))?;

    if !response.status().is_success() {
        return Err(format!("image URL returned {}", response.status()));
    }

    response
        .bytes()
        .await
        .map(|b| b.to_vec())
        .map_err(|e| format!("failed to read image bytes: {e}"))
}
